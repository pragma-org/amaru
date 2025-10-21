// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This module provides functions and data types for creating and executing a series of actions
//! exercising the chain selection logic:
//!
//!  - `Action` represents roll forward or rollback actions.
//!  - `SelectionResult` encapsulates the values returned by the `HeadersTree` on each roll forward or rollback.
//!  - `random_walk` generates a random list of actions to perform on a `HeadersTree` given a `Tree<BlockHeader>` of a given depth.
//!

use crate::consensus::errors::ConsensusError;
use crate::consensus::headers_tree::Tracker::{Me, SomePeer};
use crate::consensus::headers_tree::data_generation::SelectionResult::{Back, Forward};
use crate::consensus::headers_tree::data_generation::{GeneratedTree, any_tree_of_headers};
use crate::consensus::headers_tree::tree::Tree;
use crate::consensus::headers_tree::{HeadersTree, Tracker};
use crate::consensus::stages::select_chain::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::stages::select_chain::{ForwardChainSelection, RollbackChainSelection};
use amaru_kernel::peer::Peer;
use amaru_kernel::string_utils::ListToString;
use amaru_kernel::{HEADER_HASH_SIZE, HeaderHash, Point};
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::tests::make_header;
use amaru_ouroboros_traits::{BlockHeader, ChainStore, IsHeader};
use hex::FromHexError;
use pallas_crypto::hash::Hash;
use proptest::prelude::{Just, Strategy};
use rand::prelude::SmallRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

/// This data type models the events sent by the ChainSync mini-protocol with simplified data for the tests.
/// The serialization is adjusted to make concise string representations when transforming
/// lists of actions to JSON in order to create unit tests out of property test failures
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    RollForward { peer: Peer, header: BlockHeader },
    RollBack { peer: Peer, rollback_point: Point },
}

impl Action {
    pub fn hash(&self) -> HeaderHash {
        match self {
            Action::RollForward { header, .. } => header.hash(),
            Action::RollBack { rollback_point, .. } => rollback_point.hash(),
        }
    }

    pub fn set_peer(self, peer: &Peer) -> Self {
        match self {
            Action::RollForward { header, .. } => Action::RollForward {
                peer: peer.clone(),
                header,
            },
            Action::RollBack { rollback_point, .. } => Action::RollBack {
                peer: peer.clone(),
                rollback_point,
            },
        }
    }

    pub fn pretty_print(&self) -> String {
        format!("r#\"{}\"#", &serde_json::to_string(self).unwrap())
    }
}

struct SimplifiedHeader(BlockHeader);

impl Serialize for SimplifiedHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("BlockHeader", 4)?;
        state.serialize_field("hash", &self.0.hash())?;
        state.serialize_field("block", &self.0.block_height())?;
        state.serialize_field("slot", &self.0.slot())?;
        state.serialize_field(
            "parent",
            &self.0.parent().as_ref().map(|h| hex::encode(h.as_ref())),
        )?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for SimplifiedHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SimplifiedHeaderHelper {
            hash: String,
            block: u64,
            slot: u64,
            parent: Option<String>,
        }

        let helper = SimplifiedHeaderHelper::deserialize(deserializer)?;

        let parent_hash = if let Some(parent_str) = helper.parent {
            Some(decode_hash(parent_str.as_str()).map_err(serde::de::Error::custom)?)
        } else {
            None
        };
        let header = make_header(helper.block, helper.slot, parent_hash);
        Ok(SimplifiedHeader(BlockHeader::new(
            header,
            decode_hash(&helper.hash).map_err(serde::de::Error::custom)?,
        )))
    }
}

fn decode_hash(s: &str) -> Result<HeaderHash, FromHexError> {
    let bytes = hex::decode(s)?;
    let mut arr = [0u8; HEADER_HASH_SIZE];
    arr.copy_from_slice(&bytes);
    Ok(Hash::from(arr))
}

impl Serialize for Action {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Action::RollForward { peer, header } => ActionHelper::RollForward {
                peer: peer.to_string(),
                header: SimplifiedHeader(header.clone()),
            }
            .serialize(serializer),
            Action::RollBack {
                peer,
                rollback_point,
            } => ActionHelper::RollBack {
                peer: peer.to_string(),
                rollback_point: rollback_point.clone(),
            }
            .serialize(serializer),
        }
    }
}

#[derive(Serialize, Deserialize)]
enum ActionHelper {
    RollForward {
        peer: String,
        header: SimplifiedHeader,
    },
    RollBack {
        peer: String,
        #[serde(
            serialize_with = "serialize_point",
            deserialize_with = "deserialize_point"
        )]
        rollback_point: Point,
    },
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = ActionHelper::deserialize(deserializer)?;
        match helper {
            ActionHelper::RollForward { peer, header } => Ok(Action::RollForward {
                peer: Peer::new(&peer),
                header: header.0,
            }),
            ActionHelper::RollBack {
                peer,
                rollback_point,
            } => Ok(Action::RollBack {
                peer: Peer::new(&peer),
                rollback_point,
            }),
        }
    }
}

impl Display for Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::RollForward { peer, header } => {
                write!(f, "Forward peer {peer} to {}", header.hash())
            }
            Action::RollBack {
                peer,
                rollback_point,
            } => write!(f, "Rollback peer {peer} to {}", rollback_point.hash()),
        }
    }
}

impl Action {
    /// Return the peer of an action
    pub fn peer(&self) -> &Peer {
        match self {
            Action::RollForward { peer, .. } => peer,
            Action::RollBack { peer, .. } => peer,
        }
    }

    /// Return `true` if the action is a rollback
    pub fn is_rollback(&self) -> bool {
        matches!(self, Action::RollBack { .. })
    }
}

/// Serialize a point with an hex string for the header hash
fn serialize_point<S: Serializer>(point: &Point, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

/// Deserialize a point from the string format above
fn deserialize_point<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Point, D::Error> {
    let bytes: &str = serde::Deserialize::deserialize(deserializer)?;
    Point::try_from(bytes).map_err(serde::de::Error::custom)
}

/// This data type helps collecting the output of the execution of a list of actions
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SelectionResult {
    Forward(ForwardChainSelection<BlockHeader>),
    Back(RollbackChainSelection<BlockHeader>),
}

impl Display for SelectionResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Forward(forward) => f.write_str(&forward.to_string()),
            Back(back) => f.write_str(&back.to_string()),
        }
    }
}

/// Generate a random list of Actions for a given peer on a given tree of headers.
pub fn random_walk<R: Rng>(
    peer_rng: &mut R,
    tree: &Tree<BlockHeader>,
    peer: &Peer,
    result: &mut BTreeMap<Peer, Vec<Action>>,
) {
    if !result.contains_key(peer) {
        result.insert(peer.clone(), vec![]);
    }

    if let Some(actions) = result.get_mut(peer) {
        actions.push(Action::RollForward {
            peer: peer.clone(),
            header: tree.value.clone(),
        })
    }

    // Process the children in a random order based on a peer-specific RNG
    let mut children: Vec<_> = tree.children.clone().into_iter().collect();
    children.sort_by_key(|_c| peer_rng.random_bool(0.5));

    // Start a new random walk for each child
    for child in children.iter() {
        random_walk(peer_rng, child, peer, result);
    }

    // Come back to the parent node to explore another tree branch
    if let Some(parent) = tree.value.parent()
        && let Some(actions) = result.get_mut(peer)
    {
        let rollback = Action::RollBack {
            peer: peer.clone(),
            rollback_point: Point::Specific(tree.value.slot(), parent.to_vec()),
        };
        if actions.last().map(|h| h.hash()) != Some(rollback.hash()) {
            actions.push(rollback)
        }
    }
}

/// Generate random walks for a fixed number of peers on a given tree of headers.
///
/// The returned list of actions is transposed so that the actions from different peers are interleaved.
/// This makes sure that every peer has a chance to roll forward from the root of the tree.
/// Otherwise, if we had all the actions from peer 1, then all the actions from peer 2, etc...
/// we could end up with a tree growing beyond the `max_length` causing the tree to be pruned and
/// the root header to be removed.
pub fn generate_random_walks(generated_tree: &GeneratedTree, peers_nb: usize) -> GeneratedActions {
    let mut actions_per_peer = BTreeMap::new();

    for i in 0..peers_nb {
        let current_peer = Peer::new(&format!("{}", i + 1));
        let mut peer_rng = SmallRng::seed_from_u64(i as u64);

        random_walk(
            &mut peer_rng,
            generated_tree.tree(),
            &current_peer,
            &mut actions_per_peer,
        );
    }

    // If more than 2 peers are required, duplicate peer 2 with the actions of peer 1
    if peers_nb > 2 {
        let peer_1 = Peer::new("1");
        let peer_2 = Peer::new("2");
        let mut duplicate_actions = vec![];
        for action in actions_per_peer.get(&peer_1).cloned().unwrap_or_default() {
            duplicate_actions.push(action.set_peer(&peer_2));
        }
        actions_per_peer.insert(peer_2, duplicate_actions);
    }

    // Truncate actions to avoid a final list of rollbacks to the root of the tree
    for actions in actions_per_peer.values_mut() {
        while let Some(Action::RollBack { .. }) = actions.last() {
            actions.pop();
        }
    }

    // Transpose the actions per peer to interleave them
    let actions = transpose(actions_per_peer.values())
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    GeneratedActions {
        tree: generated_tree.clone(),
        actions,
        actions_per_peer,
    }
}

/// List of actions generated for a set of peers on a given tree of headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedActions {
    tree: GeneratedTree,
    actions_per_peer: BTreeMap<Peer, Vec<Action>>,
    actions: Vec<Action>,
}

impl GeneratedActions {
    pub fn generated_tree(&self) -> &GeneratedTree {
        &self.tree
    }

    pub fn best_chains(&self) -> Vec<Chain> {
        self.tree.best_chains()
    }

    pub fn actions(&self) -> &Vec<Action> {
        &self.actions
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn statistics(&self) -> GeneratedActionsStatistics {
        let fork_nodes = self.tree.fork_nodes();
        GeneratedActionsStatistics {
            tree_depth: self.tree.depth(),
            number_of_nodes: self.tree.nodes().len(),
            number_of_fork_nodes: fork_nodes.len(),
        }
    }
}

pub struct GeneratedActionsStatistics {
    tree_depth: usize,
    number_of_nodes: usize,
    number_of_fork_nodes: usize,
}

impl Display for GeneratedActionsStatistics {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for statistic in self.lines() {
            f.write_str(&format!("{}\n", statistic))?;
        }
        Ok(())
    }
}

impl GeneratedActionsStatistics {
    pub fn lines(&self) -> Vec<String> {
        let mut result = vec![];
        result.push(format!("Tree depth: {}", self.tree_depth));
        result.push(format!("Total number of nodes: {}", self.number_of_nodes));
        result.push(format!("Number of forks: {}", self.number_of_fork_nodes));
        result
    }
}

/// Generate a random list of actions, for a number of peers.
///
/// We first generate a tree of headers of depth `depth` with some branches.
/// Then we execute a random walk on that tree for `peers_nb` peers.
pub fn any_select_chains(depth: usize, peers_nb: usize) -> impl Strategy<Value = GeneratedActions> {
    any_tree_of_headers(depth)
        .prop_map(move |generated_tree| generate_random_walks(&generated_tree, peers_nb))
}

/// Generate a random list of actions, for a fixed number of peers, with a given tree of headers.
pub fn any_select_chains_from_tree(
    tree: &GeneratedTree,
    peers_nb: usize,
) -> impl Strategy<Value = GeneratedActions> {
    Just(generate_random_walks(tree, peers_nb))
}

/// Create an empty `HeadersTree` handling chains of maximum length `max_length` and
/// execute a list of actions against that tree.
///
pub fn execute_actions(
    max_length: usize,
    actions: &[Action],
    print: bool,
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let store = Arc::new(InMemConsensusStore::new());
    let mut tree = HeadersTree::new(store.clone(), max_length);
    execute_actions_on_tree(store, &mut tree, actions, print)
}

/// Execute a list of actions against a given HeadersTree.
///
/// We return a list of results from running the selection algorithm.
///
/// Inside the code the `let print = false` variable can be switched to `true` to trace
/// the execution with before / after state when debugging.
///
pub fn execute_actions_on_tree(
    store: Arc<dyn ChainStore<BlockHeader>>,
    tree: &mut HeadersTree<BlockHeader>,
    actions: &[Action],
    print: bool,
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let mut results: Vec<SelectionResult> = vec![];
    let mut diagnostics = BTreeMap::new();

    for (action_nb, action) in actions.iter().enumerate() {
        let result = match action {
            Action::RollForward { peer, header } => {
                // make sure that the header is in the store before rolling forward
                if !store.has_header(&header.hash()) {
                    store.store_header(header).unwrap();
                }
                match tree.select_roll_forward(peer, header) {
                    Ok(result) => Forward(result.clone()),
                    Err(_) => {
                        // Skip invalid actions like rolling forward a header that is not at the tip of a given peer
                        Forward(ForwardChainSelection::NoChange)
                    }
                }
            }
            Action::RollBack {
                peer,
                rollback_point,
            } => match tree.select_rollback(peer, &rollback_point.hash()) {
                Ok(result) => Back(result.clone()),
                Err(_) => {
                    // Skip invalid actions like rolling back to a point that is not in the tree.
                    Back(RollbackChainSelection::NoChange)
                }
            },
        };
        if print {
            diagnostics.insert(
                (action_nb + 1, action.clone()),
                (result.clone(), tree.clone()),
            );
        }
        results.push(result);
    }
    print_diagnostics(print, actions, &diagnostics);
    Ok(results)
}

/// Print diagnostics information about the execution of a list of actions:
///   - Each action, with its result and the tree after executing it.
///   - A code snippet to reproduce the test case with `execute_json_actions`.
fn print_diagnostics(
    print: bool,
    actions: &[Action],
    diagnostics: &BTreeMap<(usize, Action), (SelectionResult, HeadersTree<BlockHeader>)>,
) {
    if print {
        for ((action_nb, action), (result, after)) in diagnostics {
            eprintln!("Execute {action_nb}: {action}");
            eprintln!("Result: {result}");
            eprintln!("\nTree after:\n{after}");
            eprintln!("----------------------------------------");
        }
        eprintln!(
            "Reproduce the current test case with the following actions and 'execute_json_actions':\n"
        );
        let all_lines: Vec<String> = actions.iter().map(|action| action.pretty_print()).collect();
        eprintln!("[\n{}\n]", all_lines.list_to_string(",\n"));
        eprintln!("\n----------------------------------------");
    }
}

/// Execute a list of actions provided as JSON strings.
/// This output is provided by the `print_diagnostics` function above when a test case fails.
pub fn execute_json_actions(
    max_length: usize,
    actions_as_list_of_strings: &[&str],
    print: bool,
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let actions: Vec<Action> = serde_json::from_str(&format!(
        "[{}]",
        actions_as_list_of_strings
            .iter()
            .collect::<Vec<_>>()
            .list_to_string(",")
    ))
    .unwrap();
    execute_actions(max_length, &actions, print)
}

/// Type alias for a chain of headers tracked by a peer
pub type Chain = Vec<BlockHeader>;

/// This function computes the chains sent by each peer from a list of actions.
/// Once all the actions have been executed it returns the chains that are the longest.
///
/// The return value is a list of lists of chains:
///
///  - For each action we produce the resulting best chains.
///  - So that the last list of chains is the best chains after executing all the actions.
///
pub fn make_best_chains_from_actions(actions: &Vec<Action>) -> Vec<Vec<Chain>> {
    let mut all_best_chains: Vec<Vec<Chain>> = vec![];
    let mut current_chains: BTreeMap<Tracker, Chain> = BTreeMap::new();
    current_chains.insert(Me, vec![]);
    for action in actions {
        let tracker = SomePeer(action.peer().clone());
        if !current_chains.contains_key(&tracker) {
            current_chains.insert(tracker.clone(), vec![]);
        }
        let chain: &mut Chain = current_chains.get_mut(&tracker).unwrap();
        match action {
            Action::RollForward { header, .. } => {
                chain.push(header.clone());
            }
            Action::RollBack { rollback_point, .. } => {
                if let Some(rollback_position) =
                    chain.iter().position(|h| h.hash() == rollback_point.hash())
                {
                    chain.truncate(rollback_position + 1)
                }
            }
        }
        let best_length = current_chains.values().map(|c| c.len()).max().unwrap();
        let best_chains = current_chains
            .clone()
            .into_values()
            .filter(|c| c.len() == best_length)
            .collect::<Vec<Chain>>();
        current_chains.insert(Me, best_chains[0].clone());

        all_best_chains.push(best_chains);
    }
    all_best_chains
}

/// This function computes the best chain resulting of the execution of the chain selection algorithm.
/// It simply executes the results as if they were instructions for building a single chain: add a new tip,
/// rollback to a previous header, do nothing.
///
/// This function returns a list of chains, one per action, showing how the best chain evolves over time.
///
pub fn make_best_chains_from_results(results: &[SelectionResult]) -> Vec<Chain> {
    let mut best_chains: Vec<Chain> = vec![];
    let mut current_best_chain = vec![];
    for (i, event) in results.iter().enumerate() {
        match event {
            Forward(ForwardChainSelection::NewTip { tip, .. }) => {
                current_best_chain.push(tip.clone())
            }
            Forward(ForwardChainSelection::NoChange) => {}
            Forward(ForwardChainSelection::SwitchToFork(fork))
            | Back(RollbackChainSelection::SwitchToFork(fork)) => {
                let rollback_position = current_best_chain
                    .iter()
                    .position(|h| h.hash() == fork.rollback_header.hash());
                assert!(
                    rollback_position.is_some(),
                    "after the action {}, we have a rollback position that does not exist with hash {}",
                    i + 1,
                    fork.rollback_header.hash()
                );
                current_best_chain.truncate(rollback_position.unwrap() + 1);
                current_best_chain.extend(fork.fork.clone())
            }
            Back(RollbackBeyondLimit { .. }) => {}
            Back(RollbackChainSelection::NoChange) => {}
        }
        best_chains.push(current_best_chain.clone());
    }
    best_chains
}

/// Transpose a list of rows into a list of columns (even if the rows have different lengths).
fn transpose<I, R, T>(rows: I) -> Vec<Vec<T>>
where
    I: IntoIterator<Item = R>,
    R: IntoIterator<Item = T>,
{
    let mut iterators: Vec<_> = rows.into_iter().map(|r| r.into_iter()).collect();
    let mut result: Vec<Vec<T>> = vec![];

    while !iterators.is_empty() {
        let mut column = Vec::with_capacity(iterators.len());
        let mut next_iterators = Vec::with_capacity(iterators.len());

        for mut iterator in iterators {
            if let Some(x) = iterator.next() {
                column.push(x);
                next_iterators.push(iterator);
            }
        }
        if !column.is_empty() {
            result.push(column);
        }
        iterators = next_iterators;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_ouroboros_traits::tests::run;
    use std::collections::BTreeSet;

    #[test]
    fn transpose_works() {
        let rows = vec![
            vec![1, 2, 3],
            vec![4, 5],
            vec![6, 7, 8, 9],
            vec![10],
            vec![],
            vec![11, 12],
        ];
        let expected = vec![
            vec![1, 4, 6, 10, 11],
            vec![2, 5, 7, 12],
            vec![3, 8],
            vec![9],
        ];
        let result = transpose(rows);
        assert_eq!(result, expected);
    }

    #[test]
    fn generate_random_walks() {
        let generated_actions = run(any_select_chains(10, 3));
        let statistics = generated_actions.statistics();
        // uncomment for inspecting the generated tree and actions
        // let generated_tree = generated_actions.generated_tree();
        // println!("tree\n{}", generated_tree.tree());
        // println!("statistics\n{}", statistics);
        // for (peer, actions) in generated_actions.actions_per_peer.iter() {
        //     println!("peer {peer}\n{}", actions.list_to_string(",\n"));
        // }

        assert!(
            statistics.number_of_fork_nodes >= 3 && statistics.number_of_fork_nodes <= 4,
            "statistics.number_of_fork_nodes {}",
            statistics.number_of_fork_nodes
        );

        let actions_chains: Vec<String> = generated_actions
            .actions_per_peer
            .values()
            .map(|actions| {
                actions
                    .iter()
                    .map(|a: &Action| a.clone().set_peer(&Peer::new("0")))
                    .collect::<Vec<_>>()
                    .list_to_string(",\n")
            })
            .collect();
        let actions_set: BTreeSet<String> = actions_chains.iter().cloned().collect();
        assert_eq!(
            actions_set.len(),
            2,
            "there must be at least 2 peers with the same list of actions\nall actions\n{}\n\nall actions as a set\n{}",
            actions_chains.list_to_string("\n\n"),
            actions_set.list_to_string("\n\n")
        );
    }
}
