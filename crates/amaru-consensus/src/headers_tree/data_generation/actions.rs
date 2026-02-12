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

use crate::headers_tree::data_generation::shrink::Shrinkable;
use crate::{
    errors::ConsensusError,
    headers_tree::{
        HeadersTree, HeadersTreeDisplay,
        Tracker::{self, Me, SomePeer},
        data_generation::{
            GeneratedTree,
            SelectionResult::{Back, Forward},
            any_tree_of_headers,
        },
        tree::Tree,
    },
    stages::select_chain::{
        ForwardChainSelection,
        RollbackChainSelection::{self, RollbackBeyondLimit},
    },
};
use amaru_kernel::{
    BlockHeader, Hash, HeaderHash, IsHeader, Peer, Point, Slot, make_header,
    size::HEADER,
    utils::string::{ListToString, ListsToString},
};
use amaru_ouroboros_traits::{ChainStore, in_memory_consensus_store::InMemConsensusStore};
use hex::FromHexError;
use proptest::prelude::Strategy;
use rand::{Rng, SeedableRng, prelude::SmallRng};
use serde::{Deserialize, Serialize};
use serde_json::to_value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Debug, Display, Formatter},
    sync::Arc,
};

/// This data type models the events sent by the ChainSync mini-protocol with simplified data for the tests.
/// The serialization is adjusted to make concise string representations when transforming
/// lists of actions to JSON in order to create unit tests out of property test failures
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    RollForward { peer: Peer, header: BlockHeader },
    Rollback { peer: Peer, rollback_point: Point },
}

impl Action {
    pub fn hash(&self) -> HeaderHash {
        match self {
            Action::RollForward { header, .. } => header.hash(),
            Action::Rollback { rollback_point, .. } => rollback_point.hash(),
        }
    }

    pub fn parent_hash(&self) -> Option<HeaderHash> {
        match self {
            Action::RollForward { header, .. } => header.parent(),
            Action::Rollback { .. } => None,
        }
    }

    pub fn slot(&self) -> Slot {
        match self {
            Action::RollForward { header, .. } => header.slot(),
            Action::Rollback { rollback_point, .. } => rollback_point.slot_or_default(),
        }
    }

    pub fn pretty_print(&self) -> String {
        format!("r#\"{}\"#", &serde_json::to_string(self).unwrap())
    }

    pub fn set_peer(mut self, peer: &Peer) -> Self {
        match &mut self {
            Action::RollForward { peer: p, .. } => *p = peer.clone(),
            Action::Rollback { peer: p, .. } => *p = peer.clone(),
        }
        self
    }
}

struct SimplifiedHeader(BlockHeader);

impl serde::Serialize for SimplifiedHeader {
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

impl<'de> serde::Deserialize<'de> for SimplifiedHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
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
    let mut arr = [0u8; HEADER];
    arr.copy_from_slice(&bytes);
    Ok(Hash::from(arr))
}

impl serde::Serialize for Action {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Action::RollForward { peer, header } => ActionHelper::RollForward {
                peer: peer.to_string(),
                header: SimplifiedHeader(header.clone()),
            }
            .serialize(serializer),
            Action::Rollback {
                peer,
                rollback_point,
            } => ActionHelper::Rollback {
                peer: peer.to_string(),
                rollback_point: *rollback_point,
            }
            .serialize(serializer),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
enum ActionHelper {
    RollForward {
        peer: String,
        header: SimplifiedHeader,
    },
    Rollback {
        peer: String,
        #[serde(
            serialize_with = "serialize_point",
            deserialize_with = "deserialize_point"
        )]
        rollback_point: Point,
    },
}

impl<'de> serde::Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = ActionHelper::deserialize(deserializer)?;
        match helper {
            ActionHelper::RollForward { peer, header } => Ok(Action::RollForward {
                peer: Peer::new(&peer),
                header: header.0,
            }),
            ActionHelper::Rollback {
                peer,
                rollback_point,
            } => Ok(Action::Rollback {
                peer: Peer::new(&peer),
                rollback_point,
            }),
        }
    }
}

impl Display for Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::RollForward { peer, header, .. } => {
                write!(f, "Forward peer {peer} to {}", header.hash())
            }
            Action::Rollback {
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
            Action::Rollback { peer, .. } => peer,
        }
    }

    /// Return `true` if the action is a rollback
    pub fn is_rollback(&self) -> bool {
        matches!(self, Action::Rollback { .. })
    }
}

/// Serialize a point with an hex string for the header hash
fn serialize_point<S: serde::Serializer>(point: &Point, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

/// Deserialize a point from the string format above
fn deserialize_point<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Point, D::Error> {
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
    rng: &mut R,
    parent_header: Option<BlockHeader>,
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

    // Process the children in a random order based on a the rng
    let mut children: Vec<_> = tree.children.clone().into_iter().collect();
    children.sort_by_key(|_c| rng.random_bool(0.5));

    // Start a new random walk for each child
    for child in children.iter() {
        random_walk(rng, Some(tree.value.clone()), child, peer, result);
    }

    // Come back to the parent node to explore another tree branch
    if let Some(parent) = parent_header
        && let Some(actions) = result.get_mut(peer)
    {
        // The computation of the rollback point slot assumes that the slot of the parent header is
        // exactly one less than the slot of the current header.
        // TODO: remove this assumption
        let rollback = Action::Rollback {
            peer: peer.clone(),
            rollback_point: parent.point(),
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
pub fn generate_random_walks(
    seed: u64,
    generated_tree: &GeneratedTree,
    peers: &[Peer],
) -> GeneratedActions {
    let mut actions_per_peer = BTreeMap::new();
    let mut rng = &mut SmallRng::seed_from_u64(seed);

    for peer in peers {
        random_walk(
            &mut rng,
            None,
            generated_tree.tree(),
            peer,
            &mut actions_per_peer,
        );
    }

    // If more than 2 peers are required, duplicate peer 2 with the actions of peer 1
    if peers.len() > 2 {
        let peer_1 = &peers[0];
        let peer_2 = &peers[1];
        let mut duplicate_actions = vec![];
        for action in actions_per_peer.get(peer_1).cloned().unwrap_or_default() {
            duplicate_actions.push(action.set_peer(peer_2));
        }
        actions_per_peer.insert(peer_2.clone(), duplicate_actions);
    }

    // Truncate actions to avoid a final list of rollbacks to the root of the tree
    for actions in actions_per_peer.values_mut() {
        while let Some(Action::Rollback { .. }) = actions.last() {
            actions.pop();
        }
    }

    GeneratedActions {
        tree: generated_tree.clone(),
        actions_per_peer,
    }
}

/// List of actions generated for a set of peers on a given tree of headers.
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedActions {
    tree: GeneratedTree,
    actions_per_peer: BTreeMap<Peer, Vec<Action>>,
}

impl GeneratedActions {
    pub fn set_actions(&mut self, actions: Vec<Action>) {
        let actions_per_peer =
            actions
                .into_iter()
                .fold(BTreeMap::<Peer, Vec<Action>>::new(), |mut acc, action| {
                    acc.entry(action.peer().clone()).or_default().push(action);
                    acc
                });
        self.actions_per_peer = actions_per_peer;
    }

    pub fn generated_tree(&self) -> &GeneratedTree {
        &self.tree
    }

    pub fn best_chains(&self) -> Vec<Chain> {
        self.tree.best_chains()
    }

    pub fn actions_per_peer(&self) -> BTreeMap<Peer, Vec<Action>> {
        self.actions_per_peer.clone()
    }

    /// Transpose the actions per peer to interleave them
    /// so that we don't have all the actions from one peer first, then all the actions from another peer, etc...
    pub fn actions(&self) -> Vec<Action> {
        transpose(self.actions_per_peer.values())
            .into_iter()
            .flatten()
            .cloned()
            .collect()
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

impl Debug for GeneratedActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines = self.display_as_lines();
        for line in lines {
            writeln!(f, "{}", line)?;
        }
        Ok(())
    }
}

impl GeneratedActions {
    /// Return the actions as a list of lines, ready to be printed out.
    /// This is used in the Debug implementation but can also be fed to logs
    pub fn display_as_lines(&self) -> Vec<String> {
        let actions = self.actions();
        let mut result = vec![];
        result.push("ALL ACTIONS".to_string());
        for action in actions.iter() {
            result.push(Self::display_action(action))
        }

        result.push("BY PEER".to_string());
        for (peer, actions) in self.actions_per_peer.iter() {
            result.push(format!("\nActions from peer {}", peer));
            for action in actions.iter() {
                result.push(Self::display_action(action))
            }
        }

        result
    }

    /// Display a single action as a formatted string
    fn display_action(action: &Action) -> String {
        GeneratedAction::from(action.clone()).to_string()
    }
}

impl GeneratedActions {
    pub fn as_json(&self) -> serde_json::Value {
        let actions_json: Vec<serde_json::Value> = self
            .actions()
            .iter()
            .map(|action| to_value(GeneratedAction::from(action.clone())).unwrap())
            .collect();

        serde_json::json!({
            "tree": self.generated_tree().as_json(),
            "messages": actions_json,
        })
    }

    /// Export the generated entries to a JSON file at the given path.
    pub fn export_to_file(&self, path: &str) {
        use std::{fs::File, io::Write};

        let mut file = File::create(path).unwrap();
        let content = self.as_json().to_string();
        file.write_all(content.as_bytes()).unwrap();
    }
}

/// A single generated action formatted for display and serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GeneratedAction {
    message_type: String,
    src: String,
    hash: String,
    parent: String,
    slot: u64,
}

impl Display for GeneratedAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!(
            "{message_type:<3} {src} {slot:>5} {hash:>6} (parent {parent_hash:>6})",
            message_type = self.message_type,
            src = self.src,
            slot = self.slot,
            hash = self.hash,
            parent_hash = self.parent,
        ))
    }
}

impl From<Action> for GeneratedAction {
    fn from(action: Action) -> Self {
        let message_type = match action {
            Action::RollForward { .. } => "FWD",
            Action::Rollback { .. } => "BCK",
        };
        let header_hash = action.hash();
        let header_parent_hash = action.parent_hash();
        let slot = action.slot();

        GeneratedAction {
            message_type: message_type.to_string(),
            src: action.peer().to_string(),
            hash: header_hash.to_string().chars().take(6).collect(),
            parent: header_parent_hash
                .map(|h| h.to_string().chars().take(6).collect())
                .unwrap_or("n/a".to_string()),
            slot: slot.as_u64(),
        }
    }
}

impl Shrinkable for GeneratedActions {
    fn complement(&self, from: usize, to: usize) -> Self
    where
        Self: Sized,
    {
        let mut complement: Vec<Action> = Vec::new();
        let actions = self.actions();

        complement.extend_from_slice(&actions[..to]);
        if from < self.len() {
            complement.extend_from_slice(&actions[from..]);
        };
        let mut generated_actions = self.clone();
        generated_actions.set_actions(complement);
        generated_actions
    }

    fn len(&self) -> usize {
        self.actions().len()
    }
}

pub struct GeneratedActionsStatistics {
    pub tree_depth: usize,
    pub number_of_nodes: usize,
    pub number_of_fork_nodes: usize,
}

impl Display for GeneratedActionsStatistics {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for statistic in self.display_as_lines() {
            f.write_str(&format!("{}\n", statistic))?;
        }
        Ok(())
    }
}

impl GeneratedActionsStatistics {
    /// Return the statistics as a list of lines, ready to be printed out
    /// This is used in the Debug implementation but can also be fed to logs
    pub fn display_as_lines(&self) -> Vec<String> {
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
/// Then we execute a random walk on that tree for given peers.
pub fn any_select_chains(depth: usize, peers: &[Peer]) -> impl Strategy<Value = GeneratedActions> {
    any_tree_of_headers(depth).prop_flat_map(move |generated_tree| {
        (1..u64::MAX).prop_map(move |seed| generate_random_walks(seed, &generated_tree, peers))
    })
}

/// Generate a random list of actions, for a fixed number of peers, with a given tree of headers.
pub fn any_select_chains_from_tree(
    tree: &GeneratedTree,
    peers: &[Peer],
) -> impl Strategy<Value = GeneratedActions> {
    (1..u64::MAX).prop_map(move |seed| generate_random_walks(seed, tree, peers))
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
            Action::RollForward { peer, header, .. } => {
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
            Action::Rollback {
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
                (result.clone(), HeadersTreeDisplay::from(tree.clone())),
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
    diagnostics: &BTreeMap<(usize, Action), (SelectionResult, HeadersTreeDisplay<BlockHeader>)>,
) {
    if print {
        for ((action_nb, action), (result, after)) in diagnostics {
            eprintln!("Execute {action_nb}: {action}");
            eprintln!("Result: {result}");
            eprintln!("\nTree after:\n{after}");
            eprintln!("----------------------------------------");
        }
        eprintln!(
            "Reproduce the current test case with the following actions and 'check_execution':\n"
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
    let actions = actions_from_json(actions_as_list_of_strings);
    execute_actions(max_length, &actions, print)
}

pub fn actions_from_json(actions_as_list_of_strings: &[&str]) -> Vec<Action> {
    serde_json::from_str(&format!(
        "[{}]",
        actions_as_list_of_strings
            .iter()
            .collect::<Vec<_>>()
            .list_to_string(",")
    ))
    .unwrap()
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
                match (header.parent(), chain.last().map(|h| h.hash())) {
                    (Some(parent_hash), Some(last_hash)) => {
                        if parent_hash == last_hash {
                            chain.push(header.clone())
                        }
                    }
                    (None, None) => chain.push(header.clone()),
                    (Some(_), None) => {}
                    (None, Some(_)) => {}
                }
            }
            Action::Rollback { rollback_point, .. } => {
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
pub fn make_best_chains_from_results(results: &[SelectionResult]) -> Result<Vec<Chain>, String> {
    check_duplicated_forks(results)?;

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
                if rollback_position.is_none() {
                    return Err(format!(
                        "after the event {} {}, we have a rollback position that does not exist with hash {}\n\ncurrent best chain:\n{}\n\nbest chains\n{}",
                        i + 1,
                        event,
                        fork.rollback_header.hash(),
                        current_best_chain.list_to_string(",\n"),
                        best_chains.lists_to_string(",\n", "\n\n"),
                    ));
                }
                if let Some(rollback_position) = rollback_position {
                    current_best_chain.truncate(rollback_position + 1);
                }
                current_best_chain.extend(fork.fork.clone())
            }
            Back(RollbackBeyondLimit { .. }) => {}
            Back(RollbackChainSelection::NoChange) => {}
        }
        best_chains.push(current_best_chain.clone());
    }
    Ok(best_chains)
}

/// Check that there are no duplicated forks in the results.
/// This wouldn't necessarily break correctness but it would send redundant headers downstream.
fn check_duplicated_forks(results: &[SelectionResult]) -> Result<(), String> {
    let mut seen_forks = BTreeSet::new();
    for result in results {
        match result {
            Forward(ForwardChainSelection::SwitchToFork(fork)) => {
                if !seen_forks.insert(fork) {
                    return Err(format!(
                        "duplicated fork {:?} following a select_forward found in results",
                        fork,
                    ));
                }
            }
            Back(RollbackChainSelection::SwitchToFork(fork)) => {
                if !seen_forks.insert(fork) {
                    return Err(format!(
                        "duplicated fork {:?} following a select_rollback found in results",
                        fork,
                    ));
                }
            }
            Forward(_) | Back(_) => {}
        }
    }
    Ok(())
}

pub fn make_best_chain_from_results(results: &[SelectionResult]) -> Result<Chain, String> {
    let mut best_chain = vec![];
    for (i, event) in results.iter().enumerate() {
        match event {
            Forward(ForwardChainSelection::NewTip { tip, .. }) => best_chain.push(tip.clone()),
            Forward(ForwardChainSelection::NoChange) => {}
            Forward(ForwardChainSelection::SwitchToFork(fork))
            | Back(RollbackChainSelection::SwitchToFork(fork)) => {
                let rollback_position = best_chain
                    .iter()
                    .position(|h| h.hash() == fork.rollback_header.hash());
                if rollback_position.is_none() {
                    return Err(format!(
                        "after the event {} {}, we have a rollback position that does not exist with hash {}\n\ncurrent best chain:\n{}\n",
                        i + 1,
                        event,
                        fork.rollback_header.hash(),
                        best_chain.list_to_string(",\n")
                    ));
                }
                if let Some(rollback_position) = rollback_position {
                    best_chain.truncate(rollback_position + 1);
                }
                best_chain.extend(fork.fork.clone())
            }
            Back(RollbackBeyondLimit { .. }) => {}
            Back(RollbackChainSelection::NoChange) => {}
        }
    }
    Ok(best_chain)
}

/// Transpose a list of rows into a list of columns (even if the rows have different lengths).
pub fn transpose<I, R, T>(rows: I) -> Vec<Vec<T>>
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
    use crate::headers_tree::data_generation::generate_tree_of_headers;
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
    fn test_generate_random_walks() {
        let seed = 45;
        let tree = generate_tree_of_headers(seed, 10);
        let peers = (1..=3)
            .map(|i| Peer::new(&format!("peer-{i}")))
            .collect::<Vec<_>>();
        let generated_actions = generate_random_walks(seed, &tree, &peers);
        let statistics = generated_actions.statistics();
        // uncomment for inspecting the generated tree and actions
        // let generated_tree = generated_actions.generated_tree();
        // println!("tree\n{}", generated_tree.tree());
        // println!("statistics\n{}", statistics);
        // for (peer, actions) in generated_actions.actions_per_peer.iter() {
        //     println!("peer {peer}\n{}", actions.list_to_string(",\n"));
        // }

        assert!(
            statistics.number_of_fork_nodes >= 2 && statistics.number_of_fork_nodes <= 4,
            "statistics.number_of_fork_nodes {}",
            statistics.number_of_fork_nodes
        );

        let actions_chains: Vec<String> = generated_actions
            .actions_per_peer
            .values()
            .map(|actions| {
                actions
                    .iter()
                    .map(|a: &Action| a.clone().set_peer(&Peer::new("unused")))
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
