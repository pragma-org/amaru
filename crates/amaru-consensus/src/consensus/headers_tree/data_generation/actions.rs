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
//!  - `random_walk` uses a tree of headers of a given depth and
//!

use crate::consensus::headers_tree::data_generation::SelectionResult::{Back, Forward};
use crate::consensus::headers_tree::data_generation::{any_tree_of_headers, TestHeader};
use crate::consensus::headers_tree::tree::Tree;
use crate::consensus::headers_tree::Tracker::{Me, SomePeer};
use crate::consensus::headers_tree::{HeadersTree, ListToString, Tracker};
use crate::consensus::select_chain::RollbackChainSelection::{RollbackBeyondLimit, RollbackTo};
use crate::consensus::select_chain::{ForwardChainSelection, RollbackChainSelection};
use crate::ConsensusError;
use amaru_kernel::peer::Peer;
use amaru_kernel::Point;
use amaru_ouroboros_traits::IsHeader;
use proptest::prelude::Strategy;
use rand::prelude::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};

/// This data type models the events sent by the ChainSync mini-protocol with simplify data for the tests.
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
            Action::RollForward { ref peer, .. } => peer,
            Action::RollBack { ref peer, .. } => peer,
        }
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

/// This data type helps collecting the output for the execution of a list of actions
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
/// `max_length` is provided to make sure that we don't generate rollbacks that are beyond limit.
/// Not that this cannot occur but this would make the writing of assertions more difficult.
///
/// The generation is recursive:
///
///  - The current node in the tree gives rise to roll forward.
///  - For each of its children we:
///     - Create another random walk starting from that child.
///     - Randomly decide if there could be a rollback to the current node following the child's random walk.
///     - Then the next child would have its own random walk.
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
    max_length: usize,
    result: &mut Vec<Action>,
) {
    result.push(Action::RollForward {
        peer: peer.clone(),
        header: tree.value,
    });
    for child in tree.children.iter() {
        random_walk(rng, child, peer, max_length, result);
        if rng.random() && child.depth() < max_length {
            result.push(Action::RollBack {
                peer: peer.clone(),
                rollback_point: Point::Specific(0, tree.value.hash().to_vec()),
            });
        } else {
            break;
        }
    }
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
pub fn any_select_chains(depth: usize, max_length: usize) -> impl Strategy<Value = Vec<Action>> {
    any_tree_of_headers(depth).prop_flat_map(move |tree| {
        (1..u64::MAX).prop_map(move |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            let peers_nb = 4;
            let mut result = vec![];
            for i in 0..peers_nb {
                let peer = Peer::new(&format!("{}", i + 1));
                random_walk(&mut rng, &tree, &peer, max_length, &mut result);
            }
            result
        })
    })
}

/// Create an empty `HeadersTree` handling chains of maximum length `max_length` and
/// execute a list of actions against that tree.
///
/// We return a list of results from running the selection algorithm.
///
/// Inside the code the `let print = false` variable can be switched to `true` to trace
/// the execution with before / after state when debugging.
///
pub fn execute_actions(
    max_length: usize,
    actions: &[Action],
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let mut tree = HeadersTree::new(max_length, &None);
    let mut results: Vec<SelectionResult> = vec![];
    let mut diagnostics = BTreeMap::new();
    let print = false;

    for (action_nb, action) in actions.iter().enumerate() {
        let result = match action {
            Action::RollForward { peer, ref header } => {
                match tree.select_roll_forward(peer, *header) {
                    Ok(result) => Forward(result.clone()),
                    Err(e) => {
                        println!(
                            "Error while executing action {}: {} -> {:?}",
                            action_nb + 1,
                            action,
                            e
                        );
                        println!("\nHeadersTree {tree}");
                        print_diagnostics(print, actions, &diagnostics);
                        return Err(e);
                    }
                }
            }
            Action::RollBack {
                peer,
                ref rollback_point,
            } => match tree.select_rollback(peer, &rollback_point.hash()) {
                Ok(result) => Back(result.clone()),
                Err(e) => {
                    println!(
                        "Error while executing action {}: {} -> {:?}",
                        action_nb + 1,
                        action,
                        e
                    );
                    print_diagnostics(print, actions, &diagnostics);
                    return Err(e);
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

fn print_diagnostics(
    print: bool,
    actions: &[Action],
    diagnostics: &BTreeMap<(usize, Action), (SelectionResult, HeadersTree<TestHeader>)>,
) {
    if print {
        for ((action_nb, action), (result, after)) in diagnostics {
            println!("Execute {action_nb}: {action}");
            println!("Result: {result}");
            println!("\nTree after:\n{after}");
            println!("----------------------------------------");
        }
        println!("Reproduce the current test case with the following actions and 'execute_json_actions':\n");
        let all_lines: Vec<String> = actions
            .iter()
            .map(|action| format!("#r\"{}\"#r", &serde_json::to_string(action).unwrap()))
            .collect();
        println!("[\n{}\n]", all_lines.list_to_string(",\n"));
        println!("\n----------------------------------------");
    }
}

pub fn execute_json_actions(
    max_length: usize,
    actions_as_list_of_strings: &[&str],
) -> Result<Vec<SelectionResult>, ConsensusError> {
    let actions: Vec<Action> = serde_json::from_str(&format!(
        "[{}]",
        &actions_as_list_of_strings.list_to_string(",")
    ))
    .unwrap();
    execute_actions(max_length, &actions)
}

type Chain = Vec<TestHeader>;

/// This function computes the chains sent by each peer from a list of actions.
/// Once all the actions have been executed it returns the chains that are the longest.
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
/// rollback to a previous header, do nothing..
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
                assert!(rollback_position.is_some(), "after the action {}, we have a rollback position that does not exist with hash {}", i + 1, fork.rollback_point.hash());
                current_best_chain.truncate(rollback_position.unwrap() + 1);
                current_best_chain.extend(fork.fork.clone())
            }
            Back(RollbackTo(hash)) => {
                let rollback_position = current_best_chain.iter().position(|h| &h.hash() == hash);
                assert!(rollback_position.is_some(), "after the action {}, we have a rollback position that does not exist with hash {hash}", i + 1);
                current_best_chain.truncate(rollback_position.unwrap() + 1);
            }
            Back(RollbackBeyondLimit { .. }) => {}
            Back(RollbackChainSelection::NoChange) => {}
        }
        best_chains.push(current_best_chain.clone());
    }
    best_chains
}
