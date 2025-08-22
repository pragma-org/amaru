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

//! This module provides data types and functions to generate data suited to testing the `HeadersTree`:
//!
//!  - `Tree<H>` is used to eventually hold a tree of `TestHeader`s.
//!  - `generate_test_header_tree` generates such a tree by arbitrarily grafting subtrees on a chain
//!    of size `depth`.
//!
//!

use crate::consensus::headers_tree::data_generation::TestHeader;
use crate::consensus::headers_tree::HeadersTree;
use amaru_kernel::peer::Peer;
use amaru_kernel::HEADER_HASH_SIZE;
use amaru_ouroboros_traits::IsHeader;
use pallas_crypto::hash::Hash;
use proptest::prelude::{RngCore, Strategy};
use rand::prelude::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, Exp};
use std::fmt::{Debug, Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree<H> {
    pub value: H,
    pub children: Vec<Tree<H>>,
}

impl Display for Tree<TestHeader> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pretty_print())
    }
}

impl<H> Tree<H> {
    /// Return the depth of a `Tree`
    pub fn depth(&self) -> usize {
        1 + self.children.iter().map(|c| c.depth()).max().unwrap_or(0)
    }
}

impl<H: Clone> Tree<H> {
    /// Create a `Tree` with a single value
    fn make_leaf(root: &H) -> Tree<H> {
        Tree {
            value: root.clone(),
            children: vec![],
        }
    }

    /// Add a child to the current `Tree`
    fn add_child(&mut self, child: &H) -> &mut Tree<H> {
        self.children.push(Tree::make_leaf(child));
        self
    }

    /// Get the last child of a `Tree` to modify it (if there is one).
    fn get_last_child_mut(&mut self) -> Option<&mut Tree<H>> {
        let l = self.children.len();
        self.children.get_mut(l - 1)
    }
}

impl Tree<TestHeader> {
    /// This function is used to given incrementing slot numbers to `TestHeader`s and help test failures
    /// diagnosis
    fn renumber_slots(&mut self, start: u64) {
        self.value.slot = start;
        for (i, child) in self.children.iter_mut().enumerate() {
            child.renumber_slots(start + ((i + 1) as u64));
            child.value.parent = Some(self.value.hash);
        }
    }
}

/// Example of a displayed tree of `TestHeader`s:
///
/// tree a22427226377cc867d51ad3f130af08ad13451de7160efa2b23076fd782de967 (slot: 1, parent: None)
//     └── a8810f9ea39c3a6afb780859e8d8c7bc37b78e2f9b8d68d95e831ca1477e9b21 (slot: 2, parent: a22427226377cc867d51ad3f130af08ad13451de7160efa2b23076fd782de967)
//         ├── 1bc08781253f0a6a3f83f90e50cbce1763d8db5952384e4d1f429372d590cf23 (slot: 3, parent: a8810f9ea39c3a6afb780859e8d8c7bc37b78e2f9b8d68d95e831ca1477e9b21)
//         └── 1e3aba7a1f21d50037ae6bd23910a1ee09ac4e992e01938152f6d2dd43970164 (slot: 4, parent: a8810f9ea39c3a6afb780859e8d8c7bc37b78e2f9b8d68d95e831ca1477e9b21)
//             ├── cdafd666d3ab072afee793a7e1468addb4648a6cec2e103200bd73e3a9b766ee (slot: 5, parent: 1e3aba7a1f21d50037ae6bd23910a1ee09ac4e992e01938152f6d2dd43970164)
//             └── da3fc7b517b61024fcad5acd80e4e585902180d1eb16fd37ca2f07a37c4b3903 (slot: 6, parent: 1e3aba7a1f21d50037ae6bd23910a1ee09ac4e992e01938152f6d2dd43970164)
//                 └── f3d30e29217ced84e4565a767abcde0c1f5583a9c9c77da5bff5c542d0b985d8 (slot: 7, parent: da3fc7b517b61024fcad5acd80e4e585902180d1eb16fd37ca2f07a37c4b3903)
impl<H: Display> Tree<H> {
    pub fn pretty_print(&self) -> String {
        let mut out = String::new();
        self.pretty_print_with_prefix("", true, &mut out);
        out
    }

    fn pretty_print_with_prefix(&self, prefix: &str, is_last: bool, out: &mut String) {
        out.push_str(prefix);
        if !prefix.is_empty() {
            out.push_str(if is_last { "└── " } else { "├── " });
        }
        out.push_str(&self.value.to_string());
        out.push('\n');

        let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

        for (i, child) in self.children.iter().enumerate() {
            let last = i == self.children.len() - 1;
            child.pretty_print_with_prefix(&new_prefix, last, out);
        }
    }
}

/// Return a `proptest` Strategy producing a random `Tree<TestHeader>` of a given depth
pub fn any_tree_of_headers(depth: usize) -> impl Strategy<Value = Tree<TestHeader>> {
    (0..u64::MAX).prop_map(move |seed| generate_test_header_tree(depth, seed))
}

/// Generate a tree of headers of a given depth.
/// A seed is used to control the random generation of subtrees on top of a spine of length `depth`.
pub fn generate_test_header_tree(depth: usize, seed: u64) -> Tree<TestHeader> {
    let mut rng = StdRng::seed_from_u64(seed);

    let root = generate_test_header(&mut rng);
    let mut root_tree = Tree::make_leaf(&root);
    generate_test_header_subtree(&mut rng, &mut root_tree, depth - 1);
    root_tree.renumber_slots(1);
    root_tree
}

/// Given a random generator and a tree:
///
///  - Generate a spine (a chain of `TestHeaders`).
///  - Randomly add subtrees to nodes of that spine.
///  - Graft the spine on the last child of `tree`.
///
/// The depth is used so that the subtrees added to the spine don't have a
/// higher depth than the spine.
///
fn generate_test_header_subtree(rng: &mut StdRng, tree: &mut Tree<TestHeader>, depth: usize) {
    let mut spine = generate_headers(depth, rng);
    let mut current = tree;
    let mut current_size = 0;
    for n in spine.iter_mut() {
        current.add_child(n);
        n.parent = Some(current.value.hash());
        current = current.get_last_child_mut().unwrap();
        current_size += 1;
        let other_branch_depth = rng.random_range(0..(depth - current_size + 1));
        if other_branch_depth > 0 {
            generate_test_header_subtree(rng, current, other_branch_depth);
        }
    }
}

/// Generate a chain of headers anchored at a given header.
///
/// The chain is generated by creating headers with random body hash, and linking
/// them to the previous header in the chain until the desired length is reached.
pub fn generate_headers_chain(length: usize) -> Vec<TestHeader> {
    let mut rng = StdRng::seed_from_u64(42);
    generate_headers(length, &mut rng)
}

/// Generate just one header
pub fn generate_header() -> TestHeader {
    generate_headers_chain(1)[0]
}

/// Generate a random `HeadersTree` initialized with a single chain of `TestHeader`s
pub fn create_headers_tree(size: usize) -> HeadersTree<TestHeader> {
    let headers = generate_headers_chain(size);
    let mut tree = HeadersTree::new(10, &headers.first().cloned());
    let mut tail = headers.clone();
    _ = tail.remove(0);
    _ = tree.insert_headers(&tail);
    tree
}

/// Generate a `HeadersTree` with one chain and a peer at the tip.
pub fn initialize_with_peer(size: usize, peer: &Peer) -> HeadersTree<TestHeader> {
    let mut tree = create_headers_tree(size);
    let headers = tree.best_chain_fragment();
    let tip = headers.last().unwrap();
    tree.initialize_peer(peer, &tip.hash()).unwrap();
    tree
}

/// Generate a random `TestHeader`, child of the `parent` one.
pub fn make_header_with_parent(parent: &TestHeader) -> TestHeader {
    TestHeader {
        hash: random_hash(),
        slot: parent.slot + 1,
        parent: Some(parent.hash()),
    }
}

/// Generate a random Hash that could be the hash of a `H: IsHeader` value.
pub fn random_hash() -> Hash<HEADER_HASH_SIZE> {
    Hash::from(random_bytes(HEADER_HASH_SIZE).as_slice())
}

// IMPLEMENTATION

/// Generate a chain of headers anchored at a given header.
///
/// The chain is generated by creating headers with random body hash, and linking
/// them to the previous header in the chain until the desired length is reached.
fn generate_headers(length: usize, rng: &mut StdRng) -> Vec<TestHeader> {
    let mut headers: Vec<TestHeader> = Vec::new();
    let mut parent: Option<TestHeader> = None;
    // simulate block distribution on mainnet as an exponential distribution with parameter λ = 1/20
    let poi = Exp::new(0.05).unwrap();
    let next_slot: f32 = poi.sample(rng);
    for _ in 0..length {
        let mut header = generate_test_header(rng);
        header.slot = parent.map_or(0, |h| h.slot()) + (next_slot.floor() as u64);
        header.parent = parent.map(|h| h.hash());
        headers.push(header);
        parent = Some(header);
    }
    headers
}

/// Generate a single `TestHeader` but using the provided random generator.
fn generate_test_header(rng: &mut StdRng) -> TestHeader {
    TestHeader {
        hash: Hash::from(random_bytes_with_rng(HEADER_HASH_SIZE, rng).as_slice()),
        slot: 0,
        parent: None,
    }
}

/// Very simple function to generate random sequence of bytes of given length.
fn random_bytes(arg: usize) -> Vec<u8> {
    random_bytes_with_rng(arg, &mut StdRng::from_os_rng())
}

/// Very simple function to generate random sequence of bytes of given length.
fn random_bytes_with_rng(arg: usize, rng: &mut StdRng) -> Vec<u8> {
    let mut buffer = vec![0; arg];
    rng.fill_bytes(&mut buffer);
    buffer
}

mod tests {
    use super::*;

    #[test]
    fn test_generate_test_headers_tree() {
        let tree = generate_test_header_tree(5, 42);
        assert_eq!(tree.depth(), 5);
    }
}
