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

use crate::consensus::headers_tree::data_generation::{any_tree_of_headers, TestHeader, Tree};
use crate::consensus::select_chain::{ForwardChainSelection, RollbackChainSelection};
use amaru_kernel::peer::Peer;
use amaru_kernel::Point;
use amaru_ouroboros_traits::IsHeader;
use proptest::prelude::Strategy;
use rand::prelude::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

pub fn any_select_chains(depth: usize, max_length: usize) -> impl Strategy<Value=Vec<Action>> {
    any_tree_of_headers(depth).prop_flat_map(move |tree| {
        (1..u64::MAX).prop_map(move |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            let peers_nb = 3;
            let mut result = vec![];
            for i in 0..peers_nb {
                let peer = Peer::new(&format!("{}", i + 1));
                random_walk(&mut rng, &tree, &peer, max_length, &mut result);
            }
            result
        })
    })
}
