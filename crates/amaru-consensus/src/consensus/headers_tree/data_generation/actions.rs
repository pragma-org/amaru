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
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display, Formatter};

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

fn serialize_point<S: Serializer>(point: &Point, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

fn deserialize_point<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Point, D::Error> {
    let bytes: &str = serde::Deserialize::deserialize(deserializer)?;
    Point::try_from(bytes).map_err(serde::de::Error::custom)
}

impl Action {
    pub fn peer(&self) -> &Peer {
        match self {
            Action::RollForward { ref peer, .. } => peer,
            Action::RollBack { ref peer, .. } => peer,
        }
    }
}

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

pub fn execute_actions(
    max_length: usize,
    actions: &[Action],
) -> Result<BTreeMap<(usize, Action), SelectionResult>, ConsensusError> {
    let mut tree = HeadersTree::new(max_length, &None);
    let mut results: BTreeMap<(usize, Action), SelectionResult> = BTreeMap::new();
    let mut stopped_peers = BTreeSet::new();
    let print = false;

    for (action_nb, action) in actions.iter().enumerate() {
        if stopped_peers.contains(action.peer()) {
            continue;
        }
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

                // Stop executing a peer that has gone beyond the limit because its actions
                // will reference a header that does not exist anymore
                if let RollbackBeyondLimit { .. } = result {
                    stopped_peers.insert(peer.clone());
                }
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

pub fn make_best_chain_from_events(
    events: &BTreeMap<(usize, Action), SelectionResult>,
) -> Vec<TestHeader> {
    let mut result: Vec<TestHeader> = vec![];
    for event in events {
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
