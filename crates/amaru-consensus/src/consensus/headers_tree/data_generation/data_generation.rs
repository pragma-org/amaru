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
//!  - `Tree<H>` is used to eventually hold a tree of `BlockHeader`s.
//!  - `generate_header_tree` generates such a tree by arbitrarily grafting subtrees on a chain
//!    of size `depth`.
//!
//!

use crate::consensus::headers_tree::HeadersTree;
use crate::consensus::headers_tree::data_generation::Chain;
use crate::consensus::headers_tree::tree::Tree;
use amaru_kernel::is_header::tests::make_header;
use amaru_kernel::peer::Peer;
use amaru_kernel::{BlockHeader, Bytes, HEADER_HASH_SIZE, Header, HeaderHash, IsHeader};
use amaru_ouroboros::ChainStore;
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use proptest::prelude::Strategy;
use rand::prelude::StdRng;
use rand::{Rng, RngCore, SeedableRng};
use std::sync::Arc;

/// Return a `proptest` Strategy producing a random `GeneratedTree` of a given depth.
pub fn any_headers_tree(depth: usize) -> impl Strategy<Value = Tree<BlockHeader>> {
    any_tree_of_headers(depth).prop_map(|generated_tree| generated_tree.tree)
}

/// Return a `proptest` Strategy producing a random `GeneratedTree` of a given depth.
pub fn any_tree_of_headers(depth: usize) -> impl Strategy<Value = GeneratedTree> {
    (0..u64::MAX).prop_map(move |seed| generate_tree_of_headers(seed, depth))
}

/// A generated tree of `BlockHeader`s.
/// The depth that was used to generate the tree can be accessed.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedTree {
    tree: Tree<BlockHeader>,
    depth: usize,
}

impl GeneratedTree {
    pub fn tree(&self) -> &Tree<BlockHeader> {
        &self.tree
    }

    pub fn best_chains(&self) -> Vec<Chain> {
        self.tree.branches()
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn nodes(&self) -> Vec<BlockHeader> {
        self.tree.nodes()
    }

    pub fn fork_nodes(&self) -> Vec<BlockHeader> {
        self.tree.fork_nodes()
    }

    pub fn leaves(&self) -> Vec<BlockHeader> {
        self.tree.leaves()
    }

    pub fn as_json(&self) -> serde_json::Value {
        self.tree.as_json()
    }
}

/// Generate a tree of headers of a given depth.
/// A seed is used to control the random generation of subtrees on top of a spine of length `depth`.
pub fn generate_tree_of_headers(seed: u64, depth: usize) -> GeneratedTree {
    let mut rng = StdRng::seed_from_u64(seed);

    let root = generate_header(1, 1, None, &mut rng);
    let mut root_tree = Tree::make_leaf(&root);
    let mut spine = generate_header_subtree(&mut rng, &mut root_tree, depth, depth - 1);
    spine.insert(0, root);
    GeneratedTree {
        tree: root_tree,
        depth,
    }
}

/// Return only the generated tree of headers of a given depth.
pub fn generate_headers_tree(seed: u64, depth: usize) -> Tree<BlockHeader> {
    generate_tree_of_headers(seed, depth).tree
}

/// Given a random generator and a tree:
///
///  - Generate a spine (a chain of `TestHeaders`).
///  - Add subtrees to nodes of that spine.
///  - Graft the spine on the last child of `tree`.
///
/// We currently generate branches at every 1/3rd level of the spine and
/// randomly generate an additional branch at the same forking point
///
/// Slots are assigned to headers according to their depth so that 2 different forks would produce
/// headers at the same slot.
///
fn generate_header_subtree(
    rng: &mut StdRng,
    tree: &mut Tree<BlockHeader>,
    total_depth: usize,
    current_branch_expected_depth: usize,
) -> Chain {
    let header_body = tree.value.header_body().clone();
    let mut spine = generate_headers(
        current_branch_expected_depth,
        header_body.block_number,
        tree.value.slot(),
        Some(tree.value.hash()),
        rng,
    );
    let mut current = tree;
    let mut current_size = 0;

    for child in spine.iter_mut() {
        current.add_child(child);
        current = current.get_last_child_mut().unwrap();
        current_size += 1;

        // We decide to branch at 1/3rd and 2/3rds of the total depth
        let must_branch = if total_depth >= 3 {
            current_size % (total_depth / 3) == 0
        } else {
            false
        };
        if must_branch {
            let min_subtree_size = (current_branch_expected_depth - current_size) / 2;
            let max_subtree_size = current_branch_expected_depth - current_size;
            let subtree_size = rng.random_range(min_subtree_size..=max_subtree_size);
            generate_header_subtree(rng, current, total_depth, subtree_size);
            if rng.random_bool(0.2) {
                let subtree_size = rng.random_range(min_subtree_size..=max_subtree_size);
                generate_header_subtree(rng, current, total_depth, subtree_size);
            }
        }
    }
    spine
}

/// Generate a chain of headers.
///
/// The chain is generated by creating headers with random body hash, and linking
/// them to the previous header in the chain until the desired length is reached.
pub fn generate_headers_chain(length: usize) -> Vec<BlockHeader> {
    let mut rng = StdRng::seed_from_u64(42);
    generate_headers(length, 1, 1, None, &mut rng)
}

/// Generate a chain of headers anchored at a given header.
///
/// The chain is generated by creating headers with random body hash, and linking
/// them to the previous header, starting from `root`.
pub fn generate_headers_chain_from(length: usize, root: &BlockHeader) -> Vec<BlockHeader> {
    let mut rng = StdRng::seed_from_u64(42);
    generate_headers(
        length,
        root.block_height() + 1,
        root.slot() + 1,
        Some(root.hash()),
        &mut rng,
    )
}

/// Generate just one header
pub fn generate_single_header() -> BlockHeader {
    generate_headers_chain(1)[0].clone()
}

/// Generate a random `HeadersTree` initialized with a single chain of `BlockHeader`s
pub fn create_headers_tree_with_store(
    store: Arc<dyn ChainStore<BlockHeader>>,
    size: usize,
) -> HeadersTree<BlockHeader> {
    let headers = generate_headers_chain(size);
    for header in &headers {
        store.store_header(header).unwrap();
    }
    store.set_anchor_hash(&headers[0].hash()).unwrap();
    store
        .set_best_chain_hash(&headers[headers.len() - 1].hash())
        .unwrap();
    HeadersTree::new(store.clone(), 10)
}

/// Generate a random `HeadersTree` initialized with a single chain of `BlockHeader`s
pub fn create_headers_tree(size: usize) -> HeadersTree<BlockHeader> {
    create_headers_tree_with_store(Arc::new(InMemConsensusStore::new()), size)
}

/// Generate a `HeadersTree` with one chain and a peer at the tip.
pub fn initialize_with_peer(size: usize, peer: &Peer) -> HeadersTree<BlockHeader> {
    initialize_with_store_and_peer(Arc::new(InMemConsensusStore::new()), size, peer)
}

/// Generate a `HeadersTree` with one chain and a peer at the tip.
pub fn initialize_with_store_and_peer(
    store: Arc<dyn ChainStore<BlockHeader>>,
    size: usize,
    peer: &Peer,
) -> HeadersTree<BlockHeader> {
    let mut tree = create_headers_tree_with_store(store, size);
    tree.initialize_peer(peer, &tree.best_chain_tip().hash())
        .unwrap();
    tree
}

/// Generate a random `BlockHeader`, child of the `parent` one
/// and store it in the provided store.
pub fn store_header_with_parent(
    store: Arc<dyn ChainStore<BlockHeader>>,
    parent: &BlockHeader,
) -> BlockHeader {
    let mut std_rng = StdRng::from_seed([0; 32]);
    let header = generate_header(1, parent.slot() + 1, Some(parent.hash()), &mut std_rng);
    store.store_header(&header).unwrap();
    header
}

// IMPLEMENTATION

/// Generate a chain of headers anchored at a given header.
///
/// The chain is generated by creating headers with random body hash, and linking
/// them to the previous header in the chain until the desired length is reached.
fn generate_headers(
    length: usize,
    start_block: u64,
    start_slot: u64,
    parent: Option<HeaderHash>,
    rng: &mut StdRng,
) -> Vec<BlockHeader> {
    let mut headers: Vec<BlockHeader> = Vec::new();
    let mut current_parent = parent;
    for i in 1..length + 1 {
        let slot = start_slot + i as u64;
        let block_number = start_block + i as u64;
        let header = generate_header(block_number, slot, current_parent, rng);
        current_parent = Some(header.hash());
        headers.push(header.clone());
    }
    headers
}

/// Generate a single `BlockHeader` but using the provided random generator.
fn generate_header(
    block: u64,
    slot: u64,
    parent: Option<HeaderHash>,
    rng: &mut StdRng,
) -> BlockHeader {
    let mut header: Header = make_header(block, slot, parent);
    // introduce some randomness in the header so that the hash is not predictable
    header.body_signature = Bytes::from(random_bytes_with_rng(HEADER_HASH_SIZE, rng));
    BlockHeader::from(header)
}

/// Very simple function to generate random sequence of bytes of given length.
fn random_bytes_with_rng(arg: usize, rng: &mut StdRng) -> Vec<u8> {
    let mut buffer = vec![0; arg];
    rng.fill_bytes(&mut buffer);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_headers_tree() {
        let tree = generate_tree_of_headers(42, 5).tree;
        assert_eq!(tree.depth(), 5);
        check_nodes(&tree);
    }

    // HELPERS
    fn check_nodes(node: &Tree<BlockHeader>) {
        assert_eq!(
            node.value.hash(),
            BlockHeader::from(node.value.header().clone()).hash()
        );
        for child in &node.children {
            assert_eq!(child.value.parent(), Some(node.value.hash()));
            check_nodes(child);
        }
    }
}
