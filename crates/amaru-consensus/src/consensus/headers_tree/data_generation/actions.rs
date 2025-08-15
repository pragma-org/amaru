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

use crate::consensus::headers_tree::data_generation::{any_tree_of_headers, TestHeader};
use amaru_kernel::peer::Peer;
use amaru_kernel::{Point, HEADER_HASH_SIZE};
use pallas_crypto::hash::Hash;
use proptest::arbitrary::any;
use proptest::prelude::{Just, Strategy};
use std::collections::BTreeSet;
use std::fmt::Debug;
use toposort::{Dag, Toposort};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    RollForward { peer: Peer, header: TestHeader },
    RollBack { peer: Peer, rollback_point: Point },
}

impl Action {
    fn peer(&self) -> &Peer {
        match self {
            Action::RollForward { ref peer, .. } => peer,
            Action::RollBack { ref peer, .. } => peer,
        }
    }

    fn hash(&self) -> Hash<HEADER_HASH_SIZE> {
        match self {
            Action::RollForward { header, .. } => header.hash,
            Action::RollBack { rollback_point, .. } => rollback_point.hash(),
        }
    }
}

pub fn any_select_chains(depth: usize) -> impl Strategy<Value=Vec<Action>> {
    any_tree_of_headers(depth).prop_flat_map(|tree| {
        // Collect the tree chains
        let chains = tree.all_chains();

        // Associate a distinct peer to each chain and perform a topological sort
        // of all the roll forward actions necessary to create each chain
        let mut dag = Dag::new();
        let mut seen_nodes = BTreeSet::new();
        chains.into_iter().enumerate().for_each(|(i, chain)| {
            let peer = Peer::new(&format!("{}", i + 1));
            let mut parent_hash: Option<Hash<HEADER_HASH_SIZE>> = None;
            let mut parent_node: Option<Action> = None;
            for h in chain.into_iter() {
                let current_node = Action::RollForward {
                    peer: peer.clone(),
                    header: TestHeader {
                        hash: h.hash,
                        slot: h.slot,
                        parent: parent_hash,
                    },
                };
                if let Some(parent_node) = parent_node {
                    if !seen_nodes.contains(&current_node) {
                        dag.before(parent_node.clone(), current_node.clone());
                        seen_nodes.insert(current_node.clone());
                    }
                };
                parent_node = Some(current_node);
                parent_hash = Some(h.hash);
            }
        });
        let result: Vec<Vec<Action>> = dag.toposort().unwrap_or(vec![]);

        // Randomly shuffle each level of the topological sort to simulate data coming from
        // peers concurrently and flatten the resulting list of actions.

        Just(result)
            .prop_flat_map(shuffled_inner_vectors)
            .prop_map(|vs| vs.into_iter().flatten().collect())
    })
}

/// This strategy shuffles vectors inside a list of vectors
fn shuffled_inner_vectors<T: Clone + Debug>(
    values: Vec<Vec<T>>,
) -> impl Strategy<Value=Vec<Vec<T>>> {
    Just(values).prop_flat_map(|outer| {
        // create a list of indices covering all internal vectors
        let shuffles = proptest::collection::vec(
            any::<proptest::sample::Index>(),
            outer.iter().map(|v| v.len()).sum::<usize>(),
        );
        shuffles.prop_map(move |indexes| {
            let mut result = outer.clone();
            let mut offset = 0;
            for inner in &mut result {
                let inner_len = inner.len();
                let idxs = &indexes[offset..offset + inner_len];
                offset += inner_len;

                // reorder using the generated indexes
                let mut shuffled = inner.clone();
                for (i, &ix) in idxs.iter().enumerate() {
                    shuffled.swap(i, ix.index(inner_len));
                }
                *inner = shuffled;
            }
            result
        })
    })
}
