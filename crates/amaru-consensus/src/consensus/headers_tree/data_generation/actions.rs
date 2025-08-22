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
use crate::consensus::headers_tree::data_generation::{any_tree_of_headers, TestHeader, Tree};
use crate::consensus::headers_tree::HeadersTree;
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
            SelectionResult::Forward(forward) => f.write_str(&forward.to_string()),
            SelectionResult::Back(back) => f.write_str(&back.to_string()),
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
/// Return a Map where:
///
///  - Key = action number + action
///  - Value = the result of the action execution
///
/// Inside the code the `let print = false` variable can be switched to `true` to trace
/// the execution with before / after state when debugging.
///
pub fn execute_actions(
    max_length: usize,
    actions: &[Action],
) -> Result<BTreeMap<(usize, Action), SelectionResult>, ConsensusError> {
    let mut tree = HeadersTree::new(max_length, &None);
    let mut results: BTreeMap<(usize, Action), SelectionResult> = BTreeMap::new();
    let print = false;

    for (action_nb, action) in actions.iter().enumerate() {
        if print {
            println!("running action {action:?}")
        }
        match action {
            Action::RollForward { peer, ref header } => {
                if !tree.has_peer(peer) {
                    let parent = header
                        .parent
                        .unwrap_or(tree.get_root_hash().unwrap_or(Point::Origin.hash()));
                    tree.initialize_peer(peer, &parent)?;
                };
                if print {
                    println!("rolling forward for {peer} to {}", header.hash())
                };
                let result = tree.select_roll_forward(peer, *header)?;
                results.insert((action_nb, action.clone()), Forward(result.clone()));
                if print {
                    println!("the resulting event is {result:?}");
                    println!("headers tree\n{tree:?}");
                }
            }
            Action::RollBack {
                peer,
                ref rollback_point,
            } => {
                if print {
                    println!("rolling back for {peer} to {}", rollback_point.hash())
                };
                let result = tree.select_rollback(peer, &rollback_point.hash()).unwrap();
                results.insert((action_nb, action.clone()), Back(result.clone()));

                if print {
                    println!("the resulting event is {result:?}");
                    println!("headers tree\n{tree:?}")
                };
            }
        }
    }
    if print {
        println!("results {results:?}")
    };
    Ok(results)
}

/// This function computes the chains sent by each peer from a list of actions.
/// Once all the actions have been executed it returns the chains that are the longest.
///
pub fn make_best_chains_from_actions(actions: &Vec<Action>) -> Vec<Vec<TestHeader>> {
    let mut chains: BTreeMap<Peer, Vec<TestHeader>> = BTreeMap::new();
    for action in actions {
        if !chains.contains_key(action.peer()) {
            chains.insert(action.peer().clone(), vec![]);
        }
        let chain: &mut Vec<TestHeader> = chains.get_mut(action.peer()).unwrap();
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
    }
    let best_length = chains.values().map(|c| c.len()).max().unwrap();
    chains
        .into_values()
        .filter(|c| c.len() == best_length)
        .collect()
}

/// This function computes the best chain resulting of the execution of the chain selection algorithm.
/// It simply executes the results as if they were instructions for building a single chain: add a new tip,
/// rollback to a previous header, do nothing..
///
pub fn make_best_chain_from_results(
    results: &BTreeMap<(usize, Action), SelectionResult>,
) -> Vec<TestHeader> {
    let mut result: Vec<TestHeader> = vec![];
    for event in results {
        match event {
            (_action, Forward(ForwardChainSelection::NewTip { tip, .. })) => result.push(*tip),
            (_, Forward(ForwardChainSelection::NoChange)) => {}
            (action, Forward(ForwardChainSelection::SwitchToFork(fork)))
            | (action, Back(RollbackChainSelection::SwitchToFork(fork))) => {
                let rollback_position = result
                    .iter()
                    .position(|h| h.hash() == fork.rollback_point.hash());
                assert!(rollback_position.is_some(), "after the action {action:?}, we have a rollback position that does not exist with hash {}", fork.rollback_point.hash());
                result.truncate(rollback_position.unwrap() + 1);
                result.extend(fork.fork.clone())
            }
            (action, Back(RollbackTo(hash))) => {
                let rollback_position = result.iter().position(|h| &h.hash() == hash);
                assert!(rollback_position.is_some(), "after the action {action:?}, we have a rollback position that does not exist with hash {hash}");
                result.truncate(rollback_position.unwrap() + 1);
            }
            (_, Back(RollbackBeyondLimit { .. })) => {}
            (_, Back(RollbackChainSelection::NoChange)) => {}
        }
    }
    result
}
