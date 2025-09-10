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
//!  - `random_walk` generates a random list of actions to perform on a `HeadersTree` given a `Tree<TestHeader>` of a given depth.
//!

use crate::consensus::errors::ConsensusError;
use crate::consensus::headers_tree::Tracker::{Me, SomePeer};
use crate::consensus::headers_tree::data_generation::SelectionResult::{Back, Forward};
use crate::consensus::headers_tree::data_generation::{TestHeader, any_tree_of_headers};
use crate::consensus::headers_tree::tree::Tree;
use crate::consensus::headers_tree::{HeadersTree, Tracker};
use crate::consensus::stages::select_chain::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::stages::select_chain::{ForwardChainSelection, RollbackChainSelection};
use amaru_kernel::Point;
use amaru_kernel::peer::Peer;
use amaru_kernel::string_utils::ListToString;
use amaru_ouroboros_traits::IsHeader;
use itertools::Itertools;
use proptest::prelude::Strategy;
use rand::prelude::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};

/// This data type models the events sent by the ChainSync mini-protocol with simplified data for the tests.
/// The serialization is adjusted to make concise string representations when transforming
/// lists of actions to JSON in order to create unit tests out of property test failures
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub enum Action {
    RollForward {
        peer: Peer,
        header: TestHeader,
    },
    RollBack {
        peer: Peer,
        #[serde(
            serialize_with = "serialize_point",
            deserialize_with = "deserialize_point"
        )]
        rollback_point: Point,
    },
}

impl Display for Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::RollForward { peer, header } => {
                write!(f, "Forward peer {peer} to {}", header.hash)
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
    Forward(ForwardChainSelection<TestHeader>),
    Back(RollbackChainSelection<TestHeader>),
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
/// `max_length` is provided to make sure that we don't generate rollbacks that are beyond the tree root.
/// (because this would makes assertions harder to check).
///
/// The generation is recursive:
///
///  - The current node in the tree gives rise to a roll forward.
///  - For each of its children we:
///     - Create another random walk starting from that child.
///     - Randomly decide if there could be a rollback to the current node following the child's random walk.
///     - Then the next child would have its own random walk.
///
/// The children are processed in decreasing order of their depth so that we can make sure to generate
/// some branches with a maximum depth.
///
/// For example, given the tree:
///
///  +- 1
///     +- 2
///     +- 3
///        +- 4
///           +- 5
///           +- 6
///        +- 7
///     +- 8
///        +- 9
///           +- 10
///
/// We could generate (F = Forward, R = Rollback):
///
///   - F1, F2, F3, F4, F5, F6, R3, F7, R1, F8, F9, R9
///
/// In the sequence above, after generating a random walk starting from 4, we rollback to 3, then
/// generate a roll forward to 7.
/// Similarly, after generating a random walk starting from 3, we rollback to 1, then generate
/// roll forwards to 8, 9.
/// Finally there's a rollback to 9 but since 9 doesn't have any more children the overall walk stops.
///
pub fn random_walk(
    rng: &mut StdRng,
    tree: &Tree<TestHeader>,
    peer: &Peer,
    rollback_ratio: Ratio,
    result: &mut BTreeMap<Peer, Vec<Action>>,
) {
    if !result.contains_key(peer) {
        result.insert(peer.clone(), vec![]);
    }

    if let Some(actions) = result.get_mut(peer) {
        actions.push(Action::RollForward {
            peer: peer.clone(),
            header: tree.value,
        })
    }

    // Process the children in decreasing order of their depth
    for child in tree.children.iter().sorted_by_key(|c| Reverse(c.depth())) {
        random_walk(rng, child, peer, rollback_ratio, result);

        // Depending on the desired rollback ratio, add a rollback from the current node to the next
        // branch in the tree.
        if rng.random_ratio(rollback_ratio.0, rollback_ratio.1)
            && let Some(actions) = result.get_mut(peer)
        {
            actions.push(Action::RollBack {
                peer: peer.clone(),
                // We don't have the parent slot here but this is not important for the tests.
                rollback_point: Point::Specific(tree.value.slot, tree.value.hash().to_vec()),
            })
        }
    }

    if let Some(parent) = tree.value.parent()
        && let Some(actions) = result.get_mut(peer)
    {
        actions.push(Action::RollBack {
            peer: peer.clone(),
            rollback_point: Point::Specific(tree.value.slot, parent.to_vec()),
        })
    }
}

/// This ratio is used to control the data generation for the branching of the header tree +
/// the amount of rollbacks in the random walk.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Ratio(pub u32, pub u32);

/// Generate random walks for a fixed number of peers on a given tree of headers.
///
/// The returned list of actions is transposed so that the actions from different peers are interleaved.
/// This makes sure that every peer has a chance to roll forward from the root of the tree.
/// Otherwise, if we had all the actions from peer 1, then all the actions from peer 2, etc...
/// we could end up with a tree growing beyond the `max_length` causing the tree to be pruned and
/// the root header to be removed.
pub fn generate_random_walks(
    tree: &Tree<TestHeader>,
    peers_nb: usize,
    rollback_ratio: Ratio,
    seed: u64,
) -> Vec<Action> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut result = BTreeMap::new();
    for i in 0..peers_nb {
        let peer = Peer::new(&format!("{}", i + 1));
        random_walk(&mut rng, tree, &peer, rollback_ratio, &mut result);
    }

    transpose(result.values())
        .into_iter()
        .flatten()
        .cloned()
        .collect()
}

/// Generate a random list of actions, for a fixed number of peers, with:
///
///  - A tree of headers of depth `depth`.
///  - A `max_length` for how far rollback actions can go in the past.
///
/// If we use `max_length` < depth we will generate test cases where the `HeadersTree` will
/// have to be truncated after a number of roll forwards.
///
/// Important note: this function returns a prop test strategy but the random walks are generated
/// using a `StdRng` generator. This makes the generator reproducible, because the `StdGenerator`
/// is given a seed controlled by `proptest` but this makes the resulting list of actions non-shrinkable.
///
pub fn any_select_chains(
    depth: usize,
    rollback_ratio: Ratio,
) -> impl Strategy<Value = Vec<Action>> {
    any_tree_of_headers(depth, Ratio(1, 2)).prop_flat_map(move |tree| {
        (1..u64::MAX).prop_map(move |seed| generate_random_walks(&tree, 5, rollback_ratio, seed))
    })
}

/// Create an empty `HeadersTree` handling chains of maximum length `max_length` and
/// execute a list of actions against that tree.
///
pub fn execute_actions(
    max_length: usize,
    actions: &[Action],
    print: bool,
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let mut tree = HeadersTree::new_in_memory(max_length);
    execute_actions_on_tree(&mut tree, actions, print)
}

/// Execute a list of actions against a given HeadersTree.
///
/// We return a list of results from running the selection algorithm.
///
/// Inside the code the `let print = false` variable can be switched to `true` to trace
/// the execution with before / after state when debugging.
///
pub fn execute_actions_on_tree(
    tree: &mut HeadersTree<TestHeader>,
    actions: &[Action],
    print: bool,
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let mut results: Vec<SelectionResult> = vec![];
    let mut diagnostics = BTreeMap::new();

    for (action_nb, action) in actions.iter().enumerate() {
        let result = match action {
            Action::RollForward { peer, header } => match tree.select_roll_forward(peer, *header) {
                Ok(result) => Forward(result.clone()),
                Err(_) => {
                    // Skip invalid actions like rolling forward a header that is not at the tip of a given peer
                    Forward(ForwardChainSelection::NoChange)
                }
            },
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
    diagnostics: &BTreeMap<(usize, Action), (SelectionResult, HeadersTree<TestHeader>)>,
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
        let all_lines: Vec<String> = actions
            .iter()
            .map(|action| format!("#r\"{}\"#", &serde_json::to_string(action).unwrap()))
            .collect();
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
type Chain = Vec<TestHeader>;

/// This function computes the chains sent by each peer from a list of actions.
/// Once all the actions have been executed it returns the chains that are the longest.
///
/// The return value is a list of lists of chains, because it returns one list of chains per action.
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
                chain.push(*header);
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
            Forward(ForwardChainSelection::NewTip { tip, .. }) => current_best_chain.push(*tip),
            Forward(ForwardChainSelection::NoChange) => {}
            Forward(ForwardChainSelection::SwitchToFork(fork))
            | Back(RollbackChainSelection::SwitchToFork(fork)) => {
                let rollback_position = current_best_chain
                    .iter()
                    .position(|h| h.hash() == fork.rollback_point.hash());
                assert!(
                    rollback_position.is_some(),
                    "after the action {}, we have a rollback position that does not exist with hash {}",
                    i + 1,
                    fork.rollback_point.hash()
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
        let result = super::transpose(rows);
        assert_eq!(result, expected);
    }
}
