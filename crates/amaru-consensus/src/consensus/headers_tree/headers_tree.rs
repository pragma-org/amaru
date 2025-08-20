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

use crate::consensus::headers_tree::tree::Tree;
use crate::consensus::select_chain::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::select_chain::{Fork, ForwardChainSelection, RollbackChainSelection};
use crate::consensus::tip::Tip;
use crate::{ConsensusError, InvalidHeaderParentData};
use amaru_kernel::{peer::Peer, Point, HEADER_HASH_SIZE};
use amaru_ouroboros_traits::IsHeader;
#[cfg(test)]
use either::Either;
use itertools::Itertools;
use pallas_crypto::hash::Hash;
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};

type HeaderHash = Hash<HEADER_HASH_SIZE>;

/// This data type stores chains as a tree of headers.
/// It also keeps track of what is the latest tip for each peer.
///
/// The main function of this data type is to be able to always return the best chain for the current
/// tree of headers.
///
#[allow(dead_code)]
#[derive(Clone, PartialEq, Eq)]
pub struct HeadersTree<H> {
    /// The arena maintains the known headers by their hash.
    headers: BTreeMap<HeaderHash, Tip<H>>,
    /// View of the headers as a tree to be able to traverse faster from root to leaves.
    tree: Tree<Tip<H>>,
    /// Maximum size allowed for a given chain
    max_length: usize,
    /// This map maintains the tip of each peer chain.
    peers: BTreeMap<Peer, HeaderHash>,
    /// Tip of the best chains
    best_chains: Vec<HeaderHash>,
    /// Length of the best chain
    best_length: usize,
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq> Debug for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeadersTree {\n")?;
        f.write_fmt(format_args!("  headers:\n{:?}\n", &self.to_tree()))?;
        f.write_fmt(format_args!("  tree:\n{:?}\n", &self.tree))?;
        f.write_fmt(format_args!(
            "  peers: {}\n",
            &self
                .peers
                .iter()
                .map(|(p, h)| format!("{} -> {}", p, h))
                .join(", ")
        ))?;
        f.write_fmt(format_args!("  best_chain: {}\n", &self.best_chain()))?;
        f.write_fmt(format_args!(
            "  best_chains: {}\n",
            &self.best_chains.iter().map(|h| h.to_string()).join(", ")
        ))?;
        f.write_fmt(format_args!("  best_length: {}\n", &self.best_length))?;
        f.write_fmt(format_args!("  max_length: {}\n", &self.max_length))?;
        f.write_str("}\n")
    }
}

impl<H: IsHeader + Clone + Debug + Display + PartialEq + Eq> Display for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeadersTree {\n")?;
        f.write_fmt(format_args!("  headers:\n{}\n", &self.to_tree()))?;
        f.write_fmt(format_args!("  tree:\n{:?}\n", &self.tree))?;
        f.write_fmt(format_args!(
            "  peers: {}\n",
            &self
                .peers
                .iter()
                .map(|(p, h)| format!("{} -> {}", p, h))
                .join(", ")
        ))?;
        f.write_fmt(format_args!("  best_chain: {}\n", &self.best_chain()))?;
        f.write_fmt(format_args!(
            "  best_chains: {}\n",
            &self.best_chains.iter().map(|h| h.to_string()).join(", ")
        ))?;
        f.write_fmt(format_args!("  best_length: {}\n", &self.best_length))?;
        f.write_fmt(format_args!("  max_length: {}\n", &self.max_length))?;
        f.write_str("}\n")
    }
}

#[allow(dead_code)]
impl<H: IsHeader + Clone + Debug + PartialEq + Eq> HeadersTree<H> {
    /// Initialize a HeadersTree as a tree rooted in the Genesis header
    pub fn new(max_length: usize, root: &Option<H>) -> HeadersTree<H> {
        if let Some(root) = root {
            HeadersTree::create(max_length, Tip::Hdr(root.clone()))
        } else {
            HeadersTree::create(max_length, Tip::Genesis)
        }
    }

    /// Initialize a HeadersTree with a specific header as the root
    pub fn new_with_header(max_length: usize, header: &H) -> HeadersTree<H> {
        HeadersTree::create(max_length, Tip::Hdr(header.clone()))
    }

    /// Create a new HeadersTree with a specific tip as the root
    fn create(max_length: usize, header: Tip<H>) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );
        let mut headers = BTreeMap::new();
        headers.insert(header.hash(), header.clone());
        HeadersTree {
            tree: Tree::make_leaf(&header),
            headers,
            max_length,
            peers: BTreeMap::new(),
            best_chains: vec![header.hash()],
            best_length: 1,
        }
    }

    /// Add a peer and its current chain tip.
    /// This function must be invoked after the point header has been added to the tree,
    /// either during the initialization of the tree or a previous roll forward.
    pub fn initialize_peer(
        &mut self,
        peer: &Peer,
        hash: &HeaderHash,
    ) -> Result<(), ConsensusError> {
        if self.headers.contains_key(hash) {
            self.peers.insert(peer.clone(), hash.clone());
            Ok(())
        } else {
            Err(ConsensusError::UnknownPoint(*hash))
        }
    }

    /// Add a new header coming from an upstream peer to the tree.
    /// The result will be an event describing:
    ///   - No change (if the header is already known for example).
    ///   - A NewTip .
    ///   - A Fork if the chain that ends with this new header becomes the longest one.
    ///
    /// There will be errors if:
    ///   - The peer is unknown
    ///   - The header's parent is unknown
    ///
    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        header: H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        // The peer must be known
        if !self.peers.contains_key(peer) {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        }

        // The header must be a child of the peer's tip
        if !self.is_tip_child(peer, &header) {
            let e = ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer.clone(),
                forwarded: header.point(),
                actual: header.parent(),
                expected: self.get_point(peer)?,
            }));
            return Err(e);
        };

        self.insert_header(&header);
        self.peers.insert(peer.clone(), header.hash());
        self.select_best_chain_after_forward(peer, &header)
    }

    fn insert_header(&mut self, header: &H) {
        // add the new header to the map
        self.headers.insert(header.hash(), Tip::Hdr(header.clone()));
        // add the new header to the tree
        if let Some(parent_hash) = header.parent() {
            assert!(self.tree.add(parent_hash, &Tip::Hdr(header.clone())), "the header {header:?} must be added to the tree at {parent_hash}");
        }
    }


    /// Rollback to an existing header for an upstream peer.
    ///
    /// The result will be an event describing:
    ///   - No change if the rollback occurred on the best chain but the best chain is still tracked by another peer
    ///   - A successful rollback where the best chain stays the best even after rollback.
    ///   - A Fork if there is now another better chain
    ///   - The fact that the rollback point is too far in the past.
    ///
    /// There will be errors if:
    ///   - The peer is unknown
    ///
    pub fn select_rollback(
        &mut self,
        peer: &Peer,
        rollback_hash: &HeaderHash,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        // The peer must be known
        if !self.peers.contains_key(peer) {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        }

        if self.headers.contains_key(rollback_hash) {
            self.peers.insert(peer.clone(), rollback_hash.clone());
            self.select_best_chain_after_rollback(rollback_hash)
        } else {
            Ok(RollbackBeyondLimit {
                peer: peer.clone(),
                rollback_point: *rollback_hash,
                max_point: self.root_hash(self.best_chain()),
            })
        }
    }

    /// Determine what is the best chain after a forward, update the tracking data
    /// and return a forward chain selection result
    ///
    /// Make sure that arena nodes which are not part of any chain are removed from the arena.
    fn select_best_chain_after_forward(
        &mut self,
        peer: &Peer,
        tip: &H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        // If the tip extends one of the best chains
        let result = if self.best_chains.iter().any(|h| Some(*h) == tip.parent()) {
            // If the tip is extending _the_ best chain
            if tip.parent().as_ref() == self.best_chains.first() {
                self.best_chains = vec![tip.hash()];
                self.best_length += 1;
                Ok(ForwardChainSelection::NewTip {
                    peer: peer.clone(),
                    tip: tip.clone(),
                })
            } else {
                let previous_best_tip = self.best_chain_tip();
                let fork = self.make_fork(peer, &previous_best_tip, tip);
                self.best_chains = vec![tip.hash()];
                self.best_length += 1;
                Ok(ForwardChainSelection::SwitchToFork(fork))
            }
        } else {
            // If the tip is extending a chain that becomes now one of the best chains
            if self.ancestors(tip).len() == self.best_length {
                self.best_chains.push(tip.hash());
            }
            Ok(ForwardChainSelection::NoChange)
        };
        // Prune old headers if the best chain is now too long
        self.prune_headers();
        result
    }

    /// Determine what is the best chain after a rollback, update the tracking data
    /// and return a rollback chain selection result
    ///
    /// Make sure that arena nodes which are not part of any chain are removed from the arena.
    #[allow(clippy::expect_used)]
    fn select_best_chain_after_rollback(
        &mut self,
        rollback_hash: &HeaderHash,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        // 3 options:
        //
        // - The best chain is rolled back but stays the best -> rollback
        // - The best chain is rolled back and another chain becomes the best -> fork
        // - A chain that is not the best chain is rolled back -> no change
        //

        // If the best chain is rolled back
        if self.has_ancestor(self.best_chain_tip(), rollback_hash) {
            if self.best_chains.len() == 1 {
                Ok(RollbackChainSelection::RollbackTo(rollback_hash.clone()))
            } else {
                let previous_best_tip = self.best_chain_tip().clone();
                self.best_chains.remove(0);
                let best_peer = self
                    .peers
                    .iter()
                    .find(|(_p, h)| h == &self.best_chain())
                    .expect("there should be a best peer")
                    .0;
                let fork = self.make_fork(&best_peer, &previous_best_tip, self.best_chain_tip());
                Ok(RollbackChainSelection::SwitchToFork(fork))
            }
        } else {
            Ok(RollbackChainSelection::NoChange)
        }
    }

    /// Create a fork for a given peer:
    ///  - the old tip is the tip of the previous best chain
    ///  - the new tip is the tip of the new best chain
    fn make_fork(&self, best_peer: &Peer, old_tip: &H, new_tip: &H) -> Fork<H> {
        let intersection_hash = self.find_intersection_hash(&old_tip, &new_tip);

        // get all the hashes between the new tip and the forking hash
        let mut fork_fragment: Vec<H> = vec![];
        let mut current = new_tip;
        while current.hash() != intersection_hash {
            if let Some(parent) = current.parent() {
                fork_fragment.push(current.clone());
                current = self.unsafe_get_header(&parent);
            }
        }
        fork_fragment.reverse();

        // return the fork
        Fork {
            peer: best_peer.clone(),
            rollback_point: self
                .headers
                .get(&intersection_hash)
                .expect("Intersection hash must exist")
                .point(),
            fork: fork_fragment,
        }
    }

    fn unsafe_get_header(&self, hash: &HeaderHash) -> &H {
        self.get_header(hash)
            .expect(&format!("A header must exist for hash {}", hash))
    }

    fn get_header(&self, hash: &HeaderHash) -> Option<&H> {
        self.headers.get(hash).and_then(|h| h.to_header())
    }

    /// Return true if `target` is the hash  of an ancestor of `header`
    fn has_ancestor(&self, header: &H, target: &HeaderHash) -> bool {
        if let Some(parent_hash) = header.parent().as_ref() {
            parent_hash == target || self.has_ancestor(self.unsafe_get_header(parent_hash), target)
        } else {
            false
        }
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors(&self, header: &H) -> Vec<HeaderHash> {
        let mut result = vec![header.hash()];
        let mut current = header.parent();
        while let Some(parent_hash) = current {
            result.push(parent_hash);
            current = self.headers.get(&parent_hash).and_then(|h| h.parent());
        }
        result
    }

    /// Return root of the tree
    fn root_hash(&self, hash: &HeaderHash) -> HeaderHash {
        let mut current_header = self.headers.get(hash).expect("the header must exist");
        let mut parent_hash = current_header.parent();
        while let Some(parent) = parent_hash {
            if let Some(parent_header) = self.headers.get(&parent) {
                current_header = parent_header;
                parent_hash = current_header.parent();
            } else {
                break;
            }
        }
        current_header.hash()
    }

    /// Return true if the parent of the header is the tip of the peer chain
    fn is_tip_child(&self, peer: &Peer, header: &H) -> bool {
        self.peers.get(peer) == header.parent().as_ref()
    }

    /// Return true if the header has already been added to the arena (and not rolled-back)
    fn header_exists(&self, header: &H) -> bool {
        self.headers.contains_key(&header.hash())
    }

    /// Return the hash of the best header of a registered peer
    /// and return an error if the peer is not known.
    fn get_point(&self, peer: &Peer) -> Result<Point, ConsensusError> {
        Ok(self
            .headers
            .get(
                self.peers
                    .get(peer)
                    .ok_or(ConsensusError::UnknownPeer(peer.clone()))?,
            )
            .expect("Header must exist")
            .point())
    }

    /// Return the best currently known tip
    fn best_chain(&self) -> &HeaderHash {
        self.best_chains.first().unwrap()
    }

    /// Return the best currently known tips
    fn best_chains(&self) -> &Vec<HeaderHash> {
        &self.best_chains
    }

    /// Return the tip of the best chain that currently known as a header
    fn best_chain_tip(&self) -> &H {
        self.unsafe_get_header(self.best_chain())
    }

    /// Return the chain root header hash
    pub(crate) fn get_root_hash(&self) -> Option<HeaderHash> {
        self.ancestors(self.best_chain_tip()).first().cloned()
    }

    /// Return the header hash that is the least common parent between 2 headers in the tree
    #[allow(clippy::panic)]
    fn find_intersection_hash(&self, header1: &H, header2: &H) -> HeaderHash {
        let mut ancestors1 = self.ancestors(header1);
        let mut ancestors2 = self.ancestors(header2);
        ancestors1.reverse();
        ancestors2.reverse();

        ancestors1
            .into_iter()
            .zip(ancestors2)
            .take_while(|(n1, n2)| n1 == n2)
            .last()
            .map(|ns| ns.0).unwrap_or_else(|| panic!("by construction a tree must always have the same root for all chains. Found none for {} and {}", header1.hash(), header2.hash()))
    }

    #[allow(clippy::panic)]
    fn prune_headers(&mut self) {
        if self.best_length <= self.max_length {
            return;
        }

        let best_chain_fragment = self.best_chain_fragment_hashes();
        match best_chain_fragment.as_slice() {
            &[first, second, ..] => {
                println!("pruning {first:?}");
                println!("now second is the root {second:?}");
                self.headers.remove(&first);
                // now second becomes the new root and we need to delete all the subtrees that are not starting from it
                let other_roots = self.tree.children.iter().filter(|t| t.value.hash() != second).collect::<Vec<_>>();
                for other_root in other_roots {
                    for hash in other_root.hashes() {
                        self.headers.remove(&hash);
                    }
                    self.headers.remove(&other_root.value.hash());
                }
            }
            _ => (),
        };

        self.best_chains.drain(1..);
        self.best_length -= 1;
    }

    /// Return the tree representation of the headers tree.
    fn to_tree(&self) -> Tree<Tip<H>> {
        Tree::from(&self.headers)
    }
}

#[cfg(test)]
pub type SelectionEvent<H> = Either<ForwardChainSelection<H>, RollbackChainSelection<H>>;

#[cfg(test)]
/// Those functions are only used by tests
impl<H: IsHeader + Clone + Debug + PartialEq + Eq> HeadersTree<H> {
    /// Return true if the peer is known
    pub fn has_peer(&self, peer: &Peer) -> bool {
        self.peers.contains_key(peer)
    }

    /// Return the best chain fragment currently known as a list of headers.
    /// The list starts from the root.
    pub fn best_chain_fragment(&self) -> Vec<H> {
        let mut result: Vec<H> = self
            .ancestors(self.best_chain_tip())
            .iter()
            .filter_map(|h| self.get_header(h).cloned())
            .collect();
        result.reverse();
        result
    }

    /// Return the best chain fragment currently known as a list of hashes.
    /// The list starts from the root.
    fn best_chain_fragment_hashes(&self) -> Vec<HeaderHash> {
        let mut result = self.ancestors(self.best_chain_tip());
        result.reverse();
        result
    }

    /// Return the headers tree size in terms of how many headers are being tracked.
    /// This is used to check the garbage collection aspect of this data structure.
    fn size(&self) -> usize {
        self.headers.len()
    }

    /// Insert headers into the arena and return the last created node id
    /// This function is used to initialize the tree from persistent storage.
    pub(crate) fn insert_headers(&mut self, headers: &[H]) {
        for header in headers {
            self.insert_header(header);
        }
        let best_hash = headers
            .last()
            .expect("There must be at least one header")
            .hash();
        self.best_chains = vec![best_hash];
        self.best_length = self.best_length + headers.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::headers_tree::data_generation::SelectionResult::Forward;
    use crate::consensus::headers_tree::data_generation::*;
    use crate::consensus::select_chain::ForwardChainSelection::SwitchToFork;
    use crate::consensus::select_chain::RollbackChainSelection::RollbackTo;
    use amaru_kernel::ORIGIN_HASH;
    use itertools::Itertools;
    use proptest::proptest;
    use std::assert_matches::assert_matches;

    #[test]
    fn initialize_peer_on_empty_tree() {
        let mut tree: HeadersTree<TestHeader> = HeadersTree::new(10, &None);
        let peer = Peer::new("alice");
        let mut header = generate_header();
        header.parent = Some(ORIGIN_HASH);
        tree.initialize_peer(&peer, &Point::Origin.hash()).unwrap();
        tree.select_roll_forward(&peer, header).unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            &header,
            "there must be a best chain available after the first roll forward"
        );
    }

    #[test]
    fn single_chain_is_best_chain() {
        let headers = generate_headers_chain(5);
        let last = headers.last().unwrap();
        let mut tree = HeadersTree::new(10, &None);
        tree.insert_headers(&headers);

        assert_eq!(tree.best_chain_tip(), last);
        assert_eq!(tree.best_chain_fragment(), headers);
    }

    #[test]
    fn initialize_peer_on_known_point() {
        let mut tree = create_headers_tree(5);
        let headers = tree.best_chain_fragment();
        let peer = Peer::new("alice");

        let last_hash = headers[4].hash();
        tree.initialize_peer(&peer, &last_hash).unwrap();

        assert_eq!(tree.get_point(&peer).unwrap(), headers[4].point());
    }

    #[test]
    fn initialize_peer_on_unknown_point_fails() {
        let mut tree = create_headers_tree(5);

        let peer = Peer::new("alice");
        assert!(tree.initialize_peer(&peer, &random_hash()).is_err());
    }

    #[test]
    fn roll_forward_extends_best_chain() {
        let mut tree = create_headers_tree(5);
        let mut headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &tip.hash()).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        let result = tree.select_roll_forward(&peer, new_tip).unwrap();
        assert_eq!(result, ForwardChainSelection::NewTip { peer, tip: new_tip });
        assert_eq!(tree.best_chain_tip(), &new_tip);
        headers.push(new_tip);
        assert_eq!(tree.best_chain_fragment(), headers);
    }

    #[test]
    fn roll_forward_with_incorrect_parent_fails() {
        let mut tree = create_headers_tree(5);
        let headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        // create a new tip pointing to an incorrect parent (the first header of the chain in this example)
        let new_tip = make_header_with_parent(headers.first().unwrap());

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &tip.hash()).unwrap();

        // Now roll forward with the 6th block
        assert!(tree.select_roll_forward(&peer, new_tip).is_err());
    }

    #[test]
    fn roll_forward_from_unknown_peer_fails() {
        let mut tree = create_headers_tree(5);
        let headers = tree.best_chain_fragment();
        let last = *headers.last().unwrap();

        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &last.hash()).unwrap();

        // Now roll forward with an unknown peer
        let peer = Peer::new("bob");
        assert!(tree.select_roll_forward(&peer, last).is_err());
    }

    #[test]
    fn roll_forward_from_another_peer_at_tip_extends_best_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let mut headers = tree.best_chain_fragment();

        // Initialize bob with the same headers as alice
        let bob = Peer::new("bob");
        let tip = headers.last().unwrap();
        let new_tip = make_header_with_parent(tip);
        tree.initialize_peer(&bob, &tip.hash()).unwrap();

        // Roll forward with a new header from bob, on the same chain
        assert_eq!(
            tree.select_roll_forward(&bob, new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: bob,
                tip: new_tip
            }
        );
        assert_eq!(tree.best_chain_tip(), &new_tip);

        headers.push(new_tip);
        assert_eq!(tree.best_chain_fragment(), headers);
    }

    #[test]
    fn roll_forward_from_another_peer_on_a_smaller_chain_is_noop() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // Initialize bob with the less headers than alice
        let bob = Peer::new("bob");
        let middle = headers[2];
        tree.initialize_peer(&bob, &middle.hash()).unwrap();

        // Roll forward with a new header from bob
        let new_tip_for_bob = make_header_with_parent(&middle);
        assert_eq!(
            tree.select_roll_forward(&bob, new_tip_for_bob).unwrap(),
            ForwardChainSelection::NoChange
        );
        let tip = headers.last().unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            tip,
            "the current tip must not change"
        );

        assert_eq!(
            tree.best_chain_fragment(),
            headers,
            "the best chain hasn't changed"
        );
    }

    #[test]
    fn roll_forward_from_another_peer_on_the_best_chain_is_noop() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // Initialize bob with same chain than alice minus the last header
        let bob = Peer::new("bob");
        let bob_tip = headers[3];
        tree.initialize_peer(&bob, &bob_tip.hash()).unwrap();

        // Roll forward with a new header from bob
        let new_tip_for_bob = make_header_with_parent(&bob_tip);
        assert_eq!(
            tree.select_roll_forward(&bob, new_tip_for_bob).unwrap(),
            ForwardChainSelection::NoChange
        );
        let tip = headers.last().unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            tip,
            "the current tip must not change"
        );

        assert_eq!(
            tree.best_chain_fragment(),
            headers,
            "the best chain hasn't changed"
        );
    }

    #[test]
    fn roll_forward_from_another_peer_creates_a_fork_if_the_new_chain_is_the_best() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // Initialize bob with some headers common with alice + additional headers that are different
        // so that their chains have the same length
        let bob = Peer::new("bob");
        let middle = headers[2];
        tree.initialize_peer(&bob, &middle.hash()).unwrap();
        let mut new_bob_headers = rollforward_from(&mut tree, &middle, &bob, 2);

        // Adding a new header must create a fork
        let bob_new_header3 = make_header_with_parent(new_bob_headers.last().unwrap());
        new_bob_headers.push(bob_new_header3);
        let fork: Vec<TestHeader> = new_bob_headers;
        let fork = Fork {
            peer: bob.clone(),
            rollback_point: middle.point(),
            fork,
        };

        let result = tree.select_roll_forward(&bob, bob_new_header3).unwrap();
        assert_eq!(result, SwitchToFork(fork));
    }

    #[test]
    fn roll_forward_with_fork_to_a_disjoint_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let anchor = headers.first().unwrap();

        // Initialize bob with a completely different chain of the same size
        let bob = Peer::new("bob");
        tree.initialize_peer(&bob, &anchor.hash()).unwrap();
        let mut bob_headers = rollforward_from(&mut tree, anchor, &bob, 4);
        let bob_tip = bob_headers.last().unwrap();

        // Adding a new header for bob must create a fork
        let bob_new_tip = make_header_with_parent(bob_tip);
        bob_headers.push(bob_new_tip);
        let fork = Fork {
            peer: bob.clone(),
            rollback_point: anchor.point(),
            fork: bob_headers,
        };

        assert_eq!(
            tree.select_roll_forward(&bob, bob_new_tip).unwrap(),
            SwitchToFork(fork)
        );
    }

    #[test]
    fn roll_forward_beyond_max_length() {
        // create a chain at max length
        let mut tree = create_headers_tree(10);
        let mut headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        assert_eq!(
            tree.select_roll_forward(&alice, new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: new_tip
            }
        );
        assert_eq!(tree.best_chain_tip(), &new_tip);

        // The expected chain ends with the new tip but starts with the second header
        // in order to stay below max_length
        headers.push(new_tip);
        let expected = headers.split_off(1);
        assert_eq!(tree.best_chain_fragment(), expected);
    }

    #[test]
    fn roll_forward_with_one_chain_an_unreachable_anchor_must_be_dropped() {
        // create a chain at max length
        let mut tree = create_headers_tree(10);
        let headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        // Now roll forward a few times
        _ = rollforward_from(&mut tree, &tip, &alice, 5);

        // There still should be 10 headers in the tree
        assert_eq!(tree.size(), 10);
    }

    #[test]
    fn roll_forward_beyond_limit_drops_unreachable_chain() {
        // create a chain at max length
        let mut tree = create_headers_tree(10);
        let headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();
        println!("tree after alice: {tree}");

        let bob = Peer::new("bob");
        let first = headers[0];
        tree.initialize_peer(&bob, &first.hash()).unwrap();

        // Roll forward with 2 new headers from bob
        _ = rollforward_from(&mut tree, &first, &bob, 2);
        println!("tree with bob: {tree}");

        // Now roll forward alice's tip twice
        // Now bob's chain is unreachable and is dropped from the tree
        _ = rollforward_from(&mut tree, tip, &alice, 2);
        println!("tree after pruning: {tree}");

        assert_eq!(tree.size(), 10);
    }

    #[test]
    fn dangling_peers_are_removed_when_the_best_chain_grows_too_large() {
        // create a chain at max length for alice
        let mut tree = create_headers_tree(10);
        let headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        // start a chain for boot at the root
        let bob = Peer::new("bob");
        tree.initialize_peer(&bob, &headers[0].hash()).unwrap();
        _ = rollforward_from(&mut tree, &headers[0], &bob, 2);

        // Add some headers to the best chain. This should cause
        // the original root node disappear from the arena
        // and bob to stop being tracked
        _ = rollforward_from(&mut tree, tip, &alice, 4);

        // As a consequence it is not possible to add a new header for bob, it has to be
        // registered again
        let next_header = make_header_with_parent(tip);
        assert!(tree.select_roll_forward(&bob, next_header).is_err());
    }

    #[test]
    fn rollback_on_the_best_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let middle = headers[3];
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackTo(middle.hash()));
        assert_eq!(
            tree.best_chain_tip(),
            &headers[4],
            "the best chain tip stays the tip of the longest known chain"
        );
    }

    #[test]
    fn rollback_then_roll_forward_with_same_header_on_single_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // rollback
        let rollback_to = headers[3];
        tree.select_rollback(&alice, &rollback_to.hash()).unwrap();

        // roll forward again
        let result = tree.select_roll_forward(&alice, headers[4]).unwrap();
        assert_eq!(result, ForwardChainSelection::NoChange);
        assert_eq!(tree.best_chain_tip(), &headers[4])
    }

    #[test]
    fn rollback_then_roll_forward_on_the_best_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let middle = headers[2];

        // Rollback first
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackTo(middle.hash()));

        // Then roll forward
        let new_tip = make_header_with_parent(&middle);
        let result = tree.select_roll_forward(&alice, new_tip).unwrap();
        assert_eq!(result, ForwardChainSelection::NoChange);
    }

    #[test]
    fn rollback_beyond_limit() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        tree.initialize_peer(&bob, &headers[1].hash()).unwrap();

        // Bob tries to rollback on a header that's not part of its chain
        let result = tree.select_rollback(&bob, &headers[3].hash()).unwrap();
        assert_eq!(
            result,
            RollbackBeyondLimit {
                peer: bob,
                rollback_point: headers[3].hash(),
                max_point: headers[0].hash(),
            }
        );
    }

    #[test]
    fn rollback_to_the_root() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        let rollback_point = headers[0];
        assert_eq!(
            tree.select_rollback(&alice, &rollback_point.hash())
                .unwrap(),
            RollbackTo(rollback_point.hash())
        );
        assert_eq!(tree.best_chain_tip(), &rollback_point)
    }

    #[test]
    fn rollback_can_just_rolls_back_if_there_was_only_one_best_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // alice has the best chain with 5 headers
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // bob branches off on headers[1] and has now the best chain with 6 headers
        let header1 = headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &header1, &bob, 4); // 4 added headers

        // Now we have
        // 0 - 1 - 2 - 3 - 4 (alice)
        //     5 - 6 - 7 - 6 - 9 (*bob)

        // sanity check: bob chain is the longest
        assert_eq!(tree.best_chain_tip(), added_headers.last().unwrap());

        // Now bob is rolled back 2 headers. The best chain stays at 9
        // but bob is rolled backed to 7
        // 0 - 1 - 2 - 3 - 4 (alice)
        //     5 - 6 - 7 - 6 - 9 (*)
        //             ^
        //            bob
        let result = tree
            .select_rollback(&bob, &added_headers[1].hash())
            .unwrap();

        assert_eq!(result, RollbackTo(added_headers[1].hash()));
    }

    #[test]
    fn rollback_switches_if_there_is_more_than_one_best_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // alice has the best chain with 5 headers
        let mut tree = initialize_with_peer(5, &alice);
        let mut headers = tree.best_chain_fragment();

        // bob branches off on headers[1] and has now the best chain with 6 headers
        let header1 = headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &header1, &bob, 4); // 4 added headers

        // Now we have
        // 0 - 1 - 2 - 3 - 4 (alice)
        //     5 - 6 - 7 - 8 - 9 (*bob)

        // sanity check: bob chain is the longest
        assert_eq!(tree.best_chain_tip(), added_headers.last().unwrap());

        // alice adds one more header. Now bob's chain is the best
        // but both bob and alice are in the best chains list
        //
        // 0 - 1 - 2 - 3 - 4 - 10 (alice)
        //     5 - 6 - 7 - 8 - 9 (*bob)
        let alice_added_headers = rollforward_from(&mut tree, &headers[4], &alice, 1);

        // Now bob is rolled back 2 headers, and we switch to alice's chain
        // 0 - 1 - 2 - 3 - 4 - 10 (*alice)
        //     5 - 6 - 7 (bob)
        let result = tree
            .select_rollback(&bob, &added_headers[1].hash())
            .unwrap();

        // This switches the fork back to alice at the intersection point of their chains
        let mut forked: Vec<TestHeader> = headers.split_off(2);
        forked.push(alice_added_headers[0]);
        let fork = Fork {
            peer: alice,
            rollback_point: header1.point(),
            fork: forked,
        };
        assert_eq!(result, RollbackChainSelection::SwitchToFork(fork));
    }

    #[test]
    fn rollback_forks_if_we_rollforward_again_on_previous_best_chain() {
        let actions_as_list_of_strings = [
            "{\"RollForward\":{\"peer\":{\"name\":\"1\"},\"header\":{\"hash\":\"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\",\"slot\":1,\"parent\":null}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"1\"},\"header\":{\"hash\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\",\"slot\":2,\"parent\":\"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"1\"},\"header\":{\"hash\":\"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf\",\"slot\":3,\"parent\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"1\"},\"header\":{\"hash\":\"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04\",\"slot\":4,\"parent\":\"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf\"}}}",
            "{\"RollBack\":{\"peer\":{\"name\":\"1\"},\"rollback_point\":\"0.d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\"}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"1\"},\"header\":{\"hash\":\"fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0\",\"slot\":4,\"parent\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"1\"},\"header\":{\"hash\":\"e14eb3b5eeaa2dc35c584d2644757217f5f6e82f17a9eb9be0137044bb2302c5\",\"slot\":5,\"parent\":\"fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0\"}}}",
            "{\"RollBack\":{\"peer\":{\"name\":\"1\"},\"rollback_point\":\"0.fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0\"}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"2\"},\"header\":{\"hash\":\"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\",\"slot\":1,\"parent\":null}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"2\"},\"header\":{\"hash\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\",\"slot\":2,\"parent\":\"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"2\"},\"header\":{\"hash\":\"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf\",\"slot\":3,\"parent\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"2\"},\"header\":{\"hash\":\"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04\",\"slot\":4,\"parent\":\"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf\"}}}",
            "{\"RollBack\":{\"peer\":{\"name\":\"2\"},\"rollback_point\":\"0.e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\"}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"3\"},\"header\":{\"hash\":\"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\",\"slot\":1,\"parent\":null}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"3\"},\"header\":{\"hash\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\",\"slot\":2,\"parent\":\"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"3\"},\"header\":{\"hash\":\"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf\",\"slot\":3,\"parent\":\"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85\"}}}",
            "{\"RollForward\":{\"peer\":{\"name\":\"3\"},\"header\":{\"hash\":\"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04\",\"slot\":4,\"parent\":\"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf\"}}}"];

        let actions: Vec<Action> = serde_json::from_str(&format!(
            "[{}]",
            &actions_as_list_of_strings.iter().join(",")
        ))
            .unwrap();
        let results = execute_actions(10, &actions).unwrap();
        assert_matches!(results.values().last(), Some(Forward(SwitchToFork(_))));
    }

    #[test]
    fn rollback_can_switch_chain_given_other_chain_is_longer_variation_2() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // alice has the best chain with 3 headers
        let mut tree = initialize_with_peer(3, &alice);
        let mut headers = tree.best_chain_fragment();

        // bob starts with the same chain, rollbacks once, then becomes the best chain with 2 new
        // roll forwards and eventually rolls back so that alice becomes the best chain again.
        tree.initialize_peer(&bob, &headers[0].hash()).unwrap();

        let _ = tree.select_roll_forward(&bob, headers[1]).unwrap();
        let _ = tree.select_roll_forward(&bob, headers[2]).unwrap();
        let _ = tree.select_rollback(&bob, &headers[1].hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &headers[1], &bob, 2);
        let _ = tree
            .select_rollback(&bob, &added_headers[0].hash())
            .unwrap();
        let result = tree.select_rollback(&bob, &headers[1].hash()).unwrap();
        let intersection = headers[1].point();
        let forked_headers = headers.split_off(2);
        let fork = Fork {
            peer: alice,
            rollback_point: intersection,
            fork: forked_headers,
        };
        assert_eq!(result, RollbackChainSelection::SwitchToFork(fork));
    }

    #[test]
    fn rollback_no_switch_on_an_equal_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // 0 - 1 - 2 - 3 - 4 (alice)
        //         + - 5 - 6 (bob)
        //

        // alice has the best chain with 5 headers
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // bob has a chain of same size, branching off 2
        tree.initialize_peer(&bob, &headers[2].hash()).unwrap();
        let _added_headers = rollforward_from(&mut tree, &headers[2], &bob, 2);

        println!("tree before {tree:?}");
        // alice rolls back to 2, there is no change, even if now bob has a longer chain than
        // alice because an existing chain (0 -> 4) has the same size as bob's chain.
        let result = tree.select_rollback(&alice, &headers[2].hash()).unwrap();
        println!("tree after {tree:?}");
        assert_eq!(result, RollbackChainSelection::NoChange);
    }

    #[test]
    fn rollback_no_switch_on_a_shorter_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // 0 - 1 - 2 - 3 - 4 (alice)
        //     + - 5 - 6 (bob)

        // alice has the best chain with 5 headers
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // bob has a smaller chain, branching off 1
        tree.initialize_peer(&bob, &headers[1].hash()).unwrap();
        let _added_headers = rollforward_from(&mut tree, &headers[1], &bob, 2);

        // alice rolls back to 2, there is no change, even if now bob has a longer chain than
        // alice because an existing chain (0 -> 4) is still longer than bob's chain.
        let result = tree.select_rollback(&alice, &headers[2].hash()).unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);
    }

    #[test]
    #[should_panic(
        expected = "Cannot create a headers tree with maximum chain length lower than 2"
    )]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<TestHeader>::new(1, &None);
    }

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(1000).with_seed(42).end())]
        #[test]
        fn run_chain_selection(actions in any_select_chains(20, 10)) {
            let print = false;
            let max_length = 20;
            let results = execute_actions(max_length, &actions).unwrap();
            let actual = make_best_chain_from_results(&results);
            let expected_best_chains = make_best_chains_from_actions(&actions);
            if print {
                let all_lines: Vec<_> = actions.iter().map(|action| serde_json::to_string(&serde_json::to_string(action).unwrap()).unwrap()).collect();
                println!("[{}]", all_lines.iter().join(",\n"));
            }
            assert!(expected_best_chains.contains(&actual), "The actual chain is {actual:?}\n The best chains are {expected_best_chains:?}");
            if print { println!("TEST OK!!!!") }
        }
    }

    // HELPERS

    /// Roll forward the tree:
    ///  - Starting from a given header,
    ///  - For a given peer
    ///  - A number of times
    ///
    /// And return the last created header
    pub fn rollforward_from(
        tree: &mut HeadersTree<TestHeader>,
        header: &TestHeader,
        peer: &Peer,
        times: u32,
    ) -> Vec<TestHeader> {
        let mut result = vec![];
        let mut parent = *header;
        for _ in 0..times {
            let next_header = make_header_with_parent(&parent);
            tree.select_roll_forward(peer, next_header).unwrap();
            result.push(next_header);
            parent = next_header;
        }
        result
    }
}
