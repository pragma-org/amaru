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

use crate::consensus::chain_selection::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::chain_selection::{Fork, ForwardChainSelection, RollbackChainSelection};
use crate::consensus::headers_tree::arena::{
    get_arena_active_nodes, get_arena_root, pretty_print_root, trim_arena_unused_nodes,
};
use crate::consensus::select_chain::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::select_chain::{Fork, ForwardChainSelection, RollbackChainSelection};
use crate::consensus::tip::Tip;
use crate::{ConsensusError, InvalidHeaderParentData};
use amaru_kernel::{peer::Peer, Point, HEADER_HASH_SIZE};
use amaru_ouroboros_traits::IsHeader;
use indextree::{Arena, Node, NodeId};
use pallas_crypto::hash::Hash;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};
use std::iter::Filter;
use std::slice::Iter;

/// This data type stores chains as a tree of headers.
/// It also keeps track of what is the latest tip for each peer.
///
/// The main function of this data type is to be able to always return the best chain for the current
/// tree of headers.
///
#[allow(dead_code)]
#[derive(Clone, PartialEq, Eq)]
pub struct HeadersTree<H> {
    /// The arena maintains a list of headers and their parent/child relationship.
    arena: Arena<Tip<H>>,
    /// Maximum size allowed for a given chain
    max_length: usize,
    /// This map maintains the chain tracking data for each tracker
    peers: BTreeMap<Peer, NodeId>,
    /// Node id of the tip of the best chain
    best_chain: NodeId,
    /// Best peer so far
    best_peer: Option<Peer>,
}

impl<H: IsHeader + Clone + Debug> Debug for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let debug_peers: Vec<String> = self
            .peers
            .iter()
            .map(|(peer, node_id)| {
                format!(
                    "{} -> {}",
                    peer,
                    self.arena
                        .get(*node_id)
                        .filter(|n| !n.is_removed())
                        .map(|n| n.get().hash().to_string())
                        .unwrap_or("<REMOVED>".to_string()),
                )
            })
            .collect();
        let debug_best_chain = self
            .best_chain_tip()
            .map(|n| n.hash().to_string())
            .unwrap_or("GENESIS".to_string());

        f.write_str("HeadersTree {\n")?;
        f.write_fmt(format_args!("  arena:\n{}\n", &self.pretty_print()))?;
        f.write_fmt(format_args!("  peers: {}\n", &debug_peers.join(", ")))?;
        f.write_fmt(format_args!("  best_chain: {}\n", &debug_best_chain))?;
        f.write_fmt(format_args!(
            "  best_node_id: {}\n",
            &self.best_chain.to_string()
        ))?;
        f.write_fmt(format_args!(
            "  best_peer: {:?}\n",
            &self.best_peer.as_ref().map(|p| p.to_string())
        ))?;
        f.write_fmt(format_args!("  max_length: {}\n", &self.max_length))?;
        f.write_str("}\n")
    }
}

#[allow(dead_code)]
impl<H: IsHeader + Clone + Debug> HeadersTree<H> {
    /// Initialize a HeadersTree as a tree rooted in the Genesis header
    pub fn new(max_length: usize, root: &Option<H>) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );

        // Create a new arena

        // We estimate that a good starting point for the arena size is equivalent to having 2
        // disjoint (except at the root) chains.
        let capacity = max_length * 2;
        let mut arena: Arena<Tip<H>> = Arena::with_capacity(capacity);
        let root_node_id = if let Some(header) = root {
            arena.new_node(Tip::Hdr(header.clone()))
        } else {
            arena.new_node(Tip::Genesis)
        };

        HeadersTree {
            arena,
            max_length,
            peers: BTreeMap::new(),
            best_chain: root_node_id,
            best_peer: None,
        }
    }

    /// Initialize a HeadersTree as a tree rooted in the Genesis header
    pub fn new_with_header(max_length: usize, header: &H) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );

        // Create a new arena

        // We estimate that a good starting point for the arena size is equivalent to having 2
        // disjoint (except at the root) chains
        let capacity = max_length * 2;
        let mut arena: Arena<Tip<H>> = Arena::with_capacity(capacity);
        let genesis_node_id = arena.new_node(Tip::Hdr(header.clone()));
        HeadersTree {
            arena,
            max_length,
            peers: BTreeMap::new(),
            best_chain: genesis_node_id,
            best_peer: None,
        }
    }

    /// Create a HeadersTree from private members for data generation
    #[cfg(test)]
    pub fn create(
        arena: Arena<Tip<H>>,
        max_length: usize,
        peers: BTreeMap<Peer, NodeId>,
        best_chain: NodeId,
        best_peer: Option<Peer>,
    ) -> HeadersTree<H> {
        HeadersTree {
            arena,
            max_length,
            peers,
            best_chain,
            best_peer,
        }
    }

    /// Add a peer and its current chain tip.
    /// This function must be invoked after the point header has been added to the tree,
    /// either during the initialization of the tree or a previous roll forward.
    pub fn initialize_peer(
        &mut self,
        peer: &Peer,
        hash: &Hash<HEADER_HASH_SIZE>,
    ) -> Result<(), ConsensusError> {
        for node in self.get_active_nodes() {
            if &node.get().hash() == hash {
                if let Some(node_id) = self.arena.get_node_id(node) {
                    self.peers.insert(peer.clone(), node_id);
                    if self.best_peer.is_none() {
                        self.best_peer = Some(peer.clone())
                    }
                    return Ok(());
                }
            }
        }
        Err(ConsensusError::UnknownPoint(*hash))
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
        if !self.is_tip_child(peer, &header)? {
            let e = ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer.clone(),
                forwarded: header.point(),
                actual: header.parent(),
                expected: self.get_point(peer)?,
            }));
            return Err(e);
        };

        // If the header already exists in the arena
        // There's no change to the best chain but we might have to set the peer position correctly
        if self.header_exists(&header) {
            if self.get_point(peer)?.hash() != header.hash() {
                self.initialize_peer(peer, &header.hash())?;
            }
            return Ok(ForwardChainSelection::NoChange);
        };

        // Otherwise we can add the new header to the arena
        let peer_tip = self.get_tip(peer)?;
        let new_node_id = self.insert_header(header.clone(), &peer_tip);
        self.select_best_chain_after_forward(peer, header, &new_node_id)
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
        rollback_point_hash: &Hash<HEADER_HASH_SIZE>,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        // The peer must be known
        if !self.peers.contains_key(peer) {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        }

        if let Some(rollback_node_id) = self.get_rollback_node_id(peer, rollback_point_hash)? {
            self.select_best_chain_after_rollback(peer, rollback_point_hash, &rollback_node_id)
        } else {
            let root = self
                .get_root_for(peer)?
                .map(|h| h.hash())
                .unwrap_or(Point::Origin.hash());
            Ok(RollbackBeyondLimit {
                peer: peer.clone(),
                rollback_point: *rollback_point_hash,
                max_point: root,
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
        tip: H,
        node_id: &NodeId,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        // Get the best chain data before the peer change
        let (previous_best_tip, previous_best_length) = (self.best_chain(), self.best_length());

        // Update the peer node id
        self.peers.insert(peer.clone(), *node_id);

        // Compute the new best chain
        self.update_best_chain(peer)?;

        // 3 options:
        //
        // - The best chain was extended -> new tip
        // - Another shorter chain was extended but still shorter -> no change
        // - Another shorter chain was extended and now longer -> fork
        //
        let (new_best_tip, new_best_length) = (self.best_chain(), self.best_length());

        let result = if self.get_parent(&new_best_tip) == Some(previous_best_tip) {
            Ok(ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip,
            })
        } else if new_best_length > previous_best_length {
            let fork = self.make_fork(peer, previous_best_tip, self.best_chain);
            Ok(ForwardChainSelection::SwitchToFork(fork))
        } else {
            Ok(ForwardChainSelection::NoChange)
        };

        // Trim the best chain if too long
        self.prune_unreachable_nodes(node_id);
        result
    }

    /// Determine what is the best chain after a rollback, update the tracking data
    /// and return a rollback chain selection result
    ///
    /// Make sure that arena nodes which are not part of any chain are removed from the arena.
    #[allow(clippy::expect_used)]
    fn select_best_chain_after_rollback(
        &mut self,
        peer: &Peer,
        rollback_hash: &Hash<HEADER_HASH_SIZE>,
        rollback_node_id: &NodeId,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        // Get the best chain data before the peer change
        let (previous_best_tip, previous_best_length, previous_best_peer) = (
            self.best_chain(),
            self.best_length(),
            self.best_peer().cloned(),
        );

        // Update the peer node id
        self.peers.insert(peer.clone(), *rollback_node_id);

        // Compute the new best chain
        self.update_best_chain(peer)?;

        // 3 options:
        //
        // - The best chain is still the exact same -> no change
        // - Another chain becomes the best chain -> fork
        // - The previous best chain is smaller but still the best -> rollback
        let result = if self.best_length() == previous_best_length {
            Ok(RollbackChainSelection::NoChange)
        } else if previous_best_peer.as_ref() != self.best_peer() {
            let fork = self.make_fork(
                &self
                    .best_peer()
                    .cloned()
                    .expect("there has to be a best peer here since we updated the best chain"),
                previous_best_tip,
                self.best_chain,
            );
            Ok(RollbackChainSelection::SwitchToFork(fork))
        } else {
            Ok(RollbackChainSelection::RollbackTo(*rollback_hash))
        };

        // Remove unused nodes in the arena
        self.trim_unused_nodes();

        result
    }

    /// Create a fork for a given peer:
    ///  - the old tip is the tip of the previous best chain
    ///  - the new tip is the tip of the new best chain
    fn make_fork(&mut self, best_peer: &Peer, old_tip: NodeId, new_tip: NodeId) -> Fork<H> {
        let intersection_node_id = self.find_intersection_node_id(&old_tip, &new_tip);

        // get all the nodes between the new tip and the forking node
        let forked_node_ids: Vec<NodeId> = new_tip
            .ancestors(&self.arena)
            .take_while(|n| *n != intersection_node_id)
            .collect();

        // get the headers for each node and reverse their order to get them from older to younger
        let mut fork_fragment: Vec<H> = forked_node_ids
            .into_iter()
            .filter_map(|n| self.get_header(n).cloned())
            .collect();
        fork_fragment.reverse();

        // return the fork
        Fork {
            peer: best_peer.clone(),
            rollback_point: self
                .get_header(intersection_node_id)
                .map(|h| h.point())
                .unwrap_or(Point::Origin),
            fork: fork_fragment,
        }
    }

    /// Return true if the parent of the header is the tip of the peer chain
    fn is_tip_child(&self, peer: &Peer, header: &H) -> Result<bool, ConsensusError> {
        let peer_current_tip = self.get_point(peer)?;
        Ok(header.parent() == Some(peer_current_tip.hash()) || header.parent().is_none())
    }

    /// Return true if the header has already been added to the arena (and not rolled-back)
    fn header_exists(&self, header: &H) -> bool {
        self.get_active_nodes()
            .any(|n| n.get().hash() == header.hash())
    }

    /// Return true if the header has already been added to the arena (and not rolled-back)
    fn get_active_nodes(&self) -> Filter<Iter<'_, Node<Tip<H>>>, fn(&&Node<Tip<H>>) -> bool> {
        get_arena_active_nodes(&self.arena)
    }

    /// Return the tip (NodeId) of a registered peer
    /// and return an error if the peer is not known.
    fn get_tip(&self, peer: &Peer) -> Result<NodeId, ConsensusError> {
        match self.peers.get(peer) {
            Some(tip) => Ok(*tip),
            None => Err(ConsensusError::UnknownPeer(peer.clone())),
        }
    }

    /// Return the hash of the best header of a registered peer
    /// and return an error if the peer is not known.
    fn get_point(&self, peer: &Peer) -> Result<Point, ConsensusError> {
        Ok(self
            .get_header(self.get_tip(peer)?)
            .map(|h| h.point())
            .unwrap_or(Point::Origin))
    }

    /// Return the best currently known tip as a NodeId in the arena
    fn best_chain(&self) -> NodeId {
        self.best_chain
    }

    /// Return the length of the best chain
    fn best_length(&self) -> usize {
        if let Some(peer) = self.best_peer.as_ref() {
            self.get_length(peer)
        } else {
            0
        }
    }

    /// Return the length of a chain tracked by a given peer
    fn get_length(&self, peer: &Peer) -> usize {
        self.peers
            .get(peer)
            .map(|n| n.ancestors(&self.arena).count())
            .unwrap_or(0)
    }

    /// Return the peer having the best chain
    fn best_peer(&self) -> Option<&Peer> {
        self.best_peer.as_ref()
    }

    /// Return the tip of the best chain that currently known as a header
    fn best_chain_tip(&self) -> Option<&H> {
        self.arena
            .get(self.best_chain())
            .and_then(|n| n.get().to_header())
    }

    /// Update the best chain after a peer has done a roll forward or rollback
    #[allow(clippy::expect_used)]
    fn update_best_chain(&mut self, peer: &Peer) -> Result<(), ConsensusError> {
        if self.best_peer.as_ref().is_some() {
            let previous_best_tip = self.best_chain;
            let previous_best_length = previous_best_tip.ancestors(&self.arena).count();

            let (next_best_peer, new_best_node_id, new_best_length) = self
                .peers
                .iter()
                .map(|(peer, node_id)| (peer, node_id, node_id.ancestors(&self.arena).count()))
                .max_by_key(|(_, _, l)| *l)
                .expect("there has to be at least one peer since we have a best peer");

            if previous_best_length != new_best_length {
                self.best_chain = *new_best_node_id;
                self.best_peer = Some(next_best_peer.clone())
            }
        } else {
            self.best_chain = self.get_node_id_tip_for(peer)?;
            self.best_peer = Some(peer.clone())
        }
        Ok(())
    }

    /// Return the parent of a given node in the arena
    /// When the arena is empty this returns the genesis node id
    fn get_parent(&self, node_id: &NodeId) -> Option<NodeId> {
        let mut ancestors = node_id.ancestors(&self.arena);
        _ = ancestors.next();
        ancestors.next()
    }

    /// Return the chain root for a given Peer
    fn get_root_for(&self, peer: &Peer) -> Result<Option<&H>, ConsensusError> {
        let node_id = self.get_node_id_tip_for(peer)?;
        Ok(node_id
            .ancestors(&self.arena)
            .last()
            .and_then(|n| self.unsafe_get_arena_node(n).get().to_header()))
    }

    /// Return the chain root header
    fn get_root(&self) -> Option<&H> {
        if let Some(root_node_id) = self.get_root_node_id() {
            self.unsafe_get_arena_node(root_node_id).get().to_header()
        } else {
            None
        }
    }

    /// Return the chain root header hash
    fn get_root_hash(&self) -> Option<Hash<HEADER_HASH_SIZE>> {
        self.get_root().map(|r| r.hash())
    }

    /// Return the root node id for this tree if the arena is not empty
    fn get_root_node_id(&self) -> Option<NodeId> {
        get_arena_root(&self.arena)
    }

    /// Return the chain tip for a given Peer
    fn get_node_id_tip_for(&self, peer: &Peer) -> Result<NodeId, ConsensusError> {
        Ok(*self
            .peers
            .get(peer)
            .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?)
    }

    /// Return the node id that is the least common parent between 2 node ids
    #[allow(clippy::panic)]
    fn find_intersection_node_id(&self, node_id1: &NodeId, node_id2: &NodeId) -> NodeId {
        let mut ancestors1: Vec<NodeId> = node_id1.ancestors(&self.arena).collect();
        let mut ancestors2: Vec<NodeId> = node_id2.ancestors(&self.arena).collect();
        ancestors1.reverse();
        ancestors2.reverse();

        ancestors1
            .into_iter()
            .zip(ancestors2)
            .take_while(|(n1, n2)| n1 == n2)
            .last()
            .map(|ns| ns.0).unwrap_or_else(|| panic!("by construction a tree must always have the same root for all chains. Found none for {node_id1} and {node_id2}"))
    }

    /// Return the node id corresponding to the hash of a node in the arena.
    /// We expect that node to belong to the peer chain, otherwise we return an error.
    fn get_rollback_node_id(
        &mut self,
        peer: &Peer,
        rollback_point_hash: &Hash<HEADER_HASH_SIZE>,
    ) -> Result<Option<NodeId>, ConsensusError> {
        let peer_tip = self.get_node_id_tip_for(peer)?;
        for node_id in peer_tip.ancestors(&self.arena) {
            let node = self.unsafe_get_arena_node(node_id);
            if node.get().hash() == *rollback_point_hash {
                return Ok(Some(node_id));
            }
        }
        Ok(None)
    }

    /// Return an arena node when it is expected to be found
    #[allow(clippy::panic)]
    fn unsafe_get_arena_node(&self, node_id: NodeId) -> &Node<Tip<H>> {
        self.arena.get(node_id).unwrap_or_else(|| {
            panic!(
                "Node not found in the arena {}. The arena is {:?}",
                node_id, self.arena
            )
        })
    }

    /// Return a header
    fn get_header(&self, node_id: NodeId) -> Option<&H> {
        self.unsafe_get_arena_node(node_id).get().to_header()
    }

    /// Insert headers into the arena and return the last created node id
    /// This function is used to initialize the tree from persistent storage.
    pub fn insert_headers(&mut self, headers: &[H]) -> NodeId {
        let mut last_node_id: NodeId = self.get_root_node_id().unwrap();
        for header in headers {
            let new_node_id = self.arena.new_node(Tip::Hdr(header.clone()));
            last_node_id.append(new_node_id, &mut self.arena);
            last_node_id = new_node_id;
        }
        self.best_chain = last_node_id;
        self.best_chain()
    }

    /// Insert a new header in the arena and return its node id:
    fn insert_header(&mut self, header: H, parent_node_id: &NodeId) -> NodeId {
        let header_node_id = self.arena.new_node(Tip::Hdr(header.clone()));
        parent_node_id.append(header_node_id, &mut self.arena);
        header_node_id
    }

    #[allow(clippy::panic)]
    fn prune_unreachable_nodes(&mut self, node_id: &NodeId) {
        if self.best_length() <= self.max_length {
            return;
        }

        if let Some(parent_node_id) = self.get_parent(node_id) {
            let ancestors = parent_node_id
                .ancestors(&self.arena)
                .collect::<Vec<NodeId>>();

            if ancestors.len() < 2 {
                // Tree is too shallow to prune
                return;
            }

            match ancestors[ancestors.len() - 2..ancestors.len()] {
                [new_root, root] => {
                    let to_remove = root
                        .children(&self.arena)
                        .filter(|nid| *nid != new_root)
                        .collect::<Vec<NodeId>>();
                    for nid in to_remove {
                        nid.remove_subtree(&mut self.arena);
                    }
                    root.remove(&mut self.arena);
                    self.remove_dangling_peers();
                }
                _ => panic!("This chain selection tree is configured with a maximum chain length lower than 2 ({}), which does not make sense", self.max_length),
            }
        }
    }

    /// Make sure that no peer points to a removed node
    fn remove_dangling_peers(&mut self) {
        // Make sure that no peer points to a removed node
        let new_peers = self
            .peers
            .clone()
            .into_iter()
            .filter(|kv| !kv.1.is_removed(&self.arena))
            .collect::<Vec<_>>();
        self.peers = BTreeMap::from_iter(new_peers);
    }

    /// Walk through the list of all each peer tip
    /// and delete its descendants if they are not used in another chain
    fn trim_unused_nodes(&mut self) {
        let all_tips: BTreeSet<NodeId> = BTreeSet::from_iter(self.peers.values().copied());
        trim_arena_unused_nodes(&mut self.arena, all_tips);
    }

    /// Pretty-print the headers present in the arena as a tree, starting from the root of the tree
    fn pretty_print(&self) -> String {
        pretty_print_root(&self.arena)
    }
}

#[cfg(test)]
/// Those functions are only used by tests
impl<H: IsHeader + Clone + Debug> HeadersTree<H> {
    /// Return the best chain fragment currently known as a list of headers
    fn best_chain_fragment(&self) -> Vec<&H> {
        self.get_chain_fragment_from(&self.best_chain())
    }

    /// Return the chain ending with the header at node_id (sorted from older to younger).
    fn get_chain_fragment_from(&self, node_id: &NodeId) -> Vec<&H> {
        let mut chain: Vec<&H> = node_id
            .ancestors(&self.arena)
            .filter_map(|n_id| self.arena.get(n_id).and_then(|n| n.get().to_header()))
            .collect();
        chain.reverse();
        chain
    }

    /// Return the headers tree size in terms of how many headers are being tracked.
    /// This is used to check the garbage collection aspect of this data structure.
    fn size(&self) -> usize {
        self.arena.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::chain_selection::ForwardChainSelection;
    use crate::consensus::chain_selection::RollbackChainSelection::RollbackTo;
    use crate::consensus::headers_tree::data_generation::*;
    use amaru_kernel::{from_cbor, to_cbor, HEADER_HASH_SIZE};
    use proptest::arbitrary::any;
    use proptest::prelude::{Just, ProptestConfig, Strategy};
    use proptest::proptest;
    use std::fmt::Debug;
    use std::hash::Hasher;
    use toposort::{Dag, Toposort};

    #[test]
    fn empty() {
        let tree: HeadersTree<TestHeader> = HeadersTree::new(10, &None);
        assert_eq!(
            tree.best_chain_tip(),
            None,
            "there is not best chain for an empty tree yet"
        );
    }

    #[test]
    fn initialize_peer_on_empty_tree() {
        let mut tree: HeadersTree<TestHeader> = HeadersTree::new(10, &None);
        let peer = Peer::new("alice");
        let header = generate_header();
        tree.initialize_peer(&peer, &Point::Origin.hash()).unwrap();
        tree.select_roll_forward(&peer, header).unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            Some(&header),
            "there must be a best chain available after the first roll forward"
        );
    }

    #[test]
    fn single_chain_is_best_chain() {
        let headers = generate_headers_chain(5);
        let last = headers.last().unwrap();
        let mut tree = HeadersTree::new(10, &None);
        tree.insert_headers(&headers);

        assert_eq!(tree.best_chain_tip(), Some(last));
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&TestHeader>>()
        );
    }

    #[test]
    fn initialize_peer_on_known_point() {
        let (mut tree, headers) = create_headers_tree(5);

        let peer = Peer::new("alice");

        let last = headers.last().unwrap();
        tree.initialize_peer(&peer, &last.hash()).unwrap();

        assert_eq!(tree.get_point(&peer).unwrap(), last.point());
    }

    #[test]
    fn initialize_peer_on_unknown_point_fails() {
        let mut tree = create_headers_tree(5).0;

        let peer = Peer::new("alice");
        assert!(tree.initialize_peer(&peer, &random_hash()).is_err());
    }

    #[test]
    fn roll_forward_extends_best_chain() {
        let (mut tree, mut headers) = create_headers_tree(5);
        let tip = headers.last().unwrap();

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &tip.hash()).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        assert_eq!(
            tree.select_roll_forward(&peer, new_tip).unwrap(),
            ForwardChainSelection::NewTip { peer, tip: new_tip }
        );
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));
        headers.push(new_tip);
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&TestHeader>>()
        );
    }

    #[test]
    fn roll_forward_with_incorrect_parent_fails() {
        let (mut tree, headers) = create_headers_tree(5);
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
        let (mut tree, headers) = create_headers_tree(5);
        let last = headers.last().unwrap();

        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &last.hash()).unwrap();

        // Now roll forward with an unknown peer
        let peer = Peer::new("bob");
        assert!(tree.select_roll_forward(&peer, *last).is_err());
    }

    #[test]
    fn roll_forward_from_another_peer_at_tip_extends_best_chain() {
        let alice = Peer::new("alice");
        let (mut tree, mut headers) = initialize_with_peer(5, &alice);

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
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));

        headers.push(new_tip);
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&TestHeader>>()
        );
    }

    #[test]
    fn roll_forward_from_another_peer_on_a_smaller_chain_is_noop() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

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
            Some(tip),
            "the current tip must not change"
        );

        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&TestHeader>>(),
            "the best chain hasn't changed"
        );
    }

    #[test]
    fn roll_forward_from_another_peer_on_the_best_chain_is_noop() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

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
            Some(tip),
            "the current tip must not change"
        );

        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&TestHeader>>(),
            "the best chain hasn't changed"
        );
    }

    #[test]
    fn roll_forward_from_another_peer_creates_a_fork_if_the_new_chain_is_the_best() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

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

        assert_eq!(
            tree.select_roll_forward(&bob, bob_new_header3).unwrap(),
            ForwardChainSelection::SwitchToFork(fork)
        );
    }

    #[test]
    fn roll_forward_with_fork_to_a_disjoint_chain() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);
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
            ForwardChainSelection::SwitchToFork(fork)
        );
    }

    #[test]
    fn roll_forward_beyond_max_length() {
        // create a chain at max length
        let (mut tree, mut headers) = create_headers_tree(10);
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
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));

        // The expected chain ends with the new tip but starts with the second header
        // in order to stay below max_length
        headers.push(new_tip);
        let expected = headers.split_off(1);
        let expected = expected.iter().collect::<Vec<&TestHeader>>();
        assert_eq!(tree.best_chain_fragment(), expected);
    }

    #[test]
    fn roll_forward_with_one_chain_an_unreachable_anchor_must_be_dropped() {
        // create a chain at max length
        let (mut tree, headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        _ = tree.select_roll_forward(&alice, new_tip).unwrap();

        // The original chain anchor is still present in the arena but marked as removed (its data is dropped)
        assert_eq!(tree.size(), 11);

        // If we add a new header to the current chain, the arena reuses the removed node to store
        // the new header
        let new_tip = make_header_with_parent(&new_tip.clone());
        _ = tree.select_roll_forward(&alice, new_tip).unwrap();
        assert_eq!(tree.size(), 11);
    }

    #[test]
    fn roll_forward_beyond_limit_drops_unreachable_chain() {
        // create a chain at max length
        let (mut tree, headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        let bob = Peer::new("bob");
        let first = headers[0];
        tree.initialize_peer(&bob, &first.hash()).unwrap();

        // Roll forward with 2 new headers from bob
        _ = rollforward_from(&mut tree, &first, &bob, 2);

        // Now roll forward alice's tip twice
        // The tree must not grow in size because bob's arena nodes have been reused
        _ = rollforward_from(&mut tree, tip, &alice, 4);
        assert_eq!(tree.size(), 13);
    }

    #[test]
    fn dangling_peers_are_removed_when_the_best_chain_grows_too_large() {
        // create a chain at max length for alice
        let (mut tree, headers) = create_headers_tree(10);
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
        let (mut tree, headers) = initialize_with_peer(5, &alice);
        let middle = headers[3];
        let result = RollbackTo(middle.hash());
        assert_eq!(
            tree.select_rollback(&alice, &middle.hash()).unwrap(),
            result
        );
        assert_eq!(tree.best_chain_tip(), Some(&middle))
    }

    #[test]
    // TODO: this is a case of a peer "stuttering" which can be considered adversarial?
    fn rollback_then_roll_forward_with_same_header_on_single_chain() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        // rollback
        let rollback_to = headers[3];
        tree.select_rollback(&alice, &rollback_to.hash()).unwrap();

        // roll forward again
        let result = tree.select_roll_forward(&alice, headers[4]).unwrap();

        assert_eq!(
            result,
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: headers[4]
            }
        );
        assert_eq!(tree.best_chain_tip(), Some(&headers[4]))
    }

    #[test]
    fn rollback_then_roll_forward_on_the_best_chain() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);
        let middle = headers[2];

        // Rollback first
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackTo(middle.hash()));

        // Then roll forward
        let new_tip = make_header_with_parent(&middle);
        let result = tree.select_roll_forward(&alice, new_tip).unwrap();
        assert_eq!(
            result,
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: new_tip,
            }
        );
    }

    #[test]
    fn rollback_beyond_limit() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let (mut tree, headers) = initialize_with_peer(5, &alice);
        tree.initialize_peer(&bob, &headers[1].hash()).unwrap();

        // Bob tries to rollback on a header that's not part of its chain
        let result = tree.select_rollback(&bob, &headers[3].hash()).unwrap();
        assert_eq!(
            result,
            RollbackChainSelection::RollbackBeyondLimit {
                peer: bob,
                rollback_point: headers[3].hash(),
                max_point: headers[0].hash(),
            }
        );
    }

    #[test]
    fn rollback_to_root() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        let rollback_point = headers[0];
        assert_eq!(
            tree.select_rollback(&alice, &rollback_point.hash())
                .unwrap(),
            RollbackTo(rollback_point.hash())
        );
        assert_eq!(tree.best_chain_tip(), Some(&rollback_point))
    }

    #[test]
    fn rollback_to_the_root() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        let rollback_point = headers[0];
        assert_eq!(
            tree.select_rollback(&alice, &rollback_point.hash())
                .unwrap(),
            RollbackTo(rollback_point.hash())
        );
        assert_eq!(tree.best_chain_tip(), Some(&rollback_point))
    }

    #[test]
    fn rollback_can_switch_chain_given_other_chain_is_longer() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // alice has the best chain with 5 headers
        let (mut tree, mut headers) = initialize_with_peer(5, &alice);

        // bob branches off on headers[1] and has now the best chain with 6 headers
        let header1 = headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &header1, &bob, 4); // 4 added headers

        // sanity check: bob chain is the longest
        assert_eq!(
            tree.best_chain_tip().unwrap(),
            added_headers.last().unwrap()
        );

        // Now bob is rolled back 2 headers
        let result = tree
            .select_rollback(&bob, &added_headers[1].hash())
            .unwrap();

        // This switches the fork back to alice at the intersection point of their chains
        let forked_headers = headers.split_off(2);
        let fork = Fork {
            peer: alice,
            rollback_point: header1.point(),
            fork: forked_headers,
        };
        assert_eq!(result, RollbackChainSelection::SwitchToFork(fork));
    }

    #[test]
    #[should_panic(
        expected = "Cannot create a headers tree with maximum chain length lower than 2"
    )]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<TestHeader>::new(1, &None);
    }

    #[test]
    fn trim_unused_nodes_on_simple_tree() {
        let mut arena: Arena<&str> = Arena::new();
        let n_0 = arena.new_node("0");
        let n_1 = arena.new_node("1");
        let n_2 = arena.new_node("2");
        let n_2_1 = arena.new_node("2_1");
        let n_2_1_1 = arena.new_node("2_1_1");
        let n_2_2 = arena.new_node("2_2");
        let n_3 = arena.new_node("3");

        n_0.append(n_1, &mut arena);
        n_0.append(n_2, &mut arena);
        n_2.append(n_2_1, &mut arena);
        n_2_1.append(n_2_1_1, &mut arena);
        n_2.append(n_2_2, &mut arena);
        n_0.append(n_3, &mut arena);

        let peer_tips = BTreeSet::from_iter(vec![n_0, n_1, n_2, n_2_1, n_2_2]);
        trim_arena_unused_nodes(&mut arena, peer_tips);

        let active_nodes: BTreeSet<&str> =
            get_arena_active_nodes(&arena).map(|n| *n.get()).collect();
        assert_eq!(
            active_nodes,
            BTreeSet::from_iter(vec!["0", "1", "2", "2_1", "2_2"])
        )
    }

    proptest! {
        #[test]
        fn generate_tree(tree in any_headers_tree(3, 5, 2)) {
            assert_eq!(tree.best_length(), 3);
        }
    }

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(1).end())]
        #[test]
        fn simulate_peer((upstream_tree, actions) in any_roll_forward_actions(5, 10)) {
            let mut our_tree = HeadersTree::new(10, &upstream_tree.get_root().cloned());
            let results = execute_roll_forward_actions(&mut our_tree, actions);
            assert_eq!(our_tree.best_length(), upstream_tree.best_length());

            let mut downstream_tree = HeadersTree::new(10, &our_tree.get_root().cloned());

            for result in results {
                match result {
                    ForwardChainSelection::NewTip{ peer, tip } => {
                        if !downstream_tree.peers.keys().collect::<Vec<_>>().contains(&&peer) {
                            if let Some(parent) = tip.parent {
                                downstream_tree.initialize_peer(&peer, &parent)?;
                            }
                        };
                        _ = downstream_tree.select_roll_forward(&peer, tip)?

                    },
                    ForwardChainSelection::SwitchToFork(fork) => {
                        if !downstream_tree.peers.keys().collect::<Vec<_>>().contains(&&fork.peer) {
                            downstream_tree.initialize_peer(&fork.peer, &fork.rollback_point.hash())?;
                        };
                        _ = downstream_tree.select_rollback(&fork.peer, &fork.rollback_point.hash())?;
                        for h in fork.fork {
                            _ = downstream_tree.select_roll_forward(&fork.peer, h)?
                        }
                    },
                    ForwardChainSelection::NoChange => {},
               }
            }
            assert_eq!(downstream_tree.best_length(), upstream_tree.best_length());
            assert_eq!(downstream_tree.best_chain_fragment(), our_tree.best_chain_fragment());
        }
    }

    fn execute_roll_forward_actions(
        tree: &mut HeadersTree<TestHeader>,
        actions: Vec<RollForwardAction>,
    ) -> Vec<ForwardChainSelection<TestHeader>> {
        let mut initialized_peers = BTreeSet::new();
        let mut results = vec![];
        for action in actions.into_iter() {
            if !initialized_peers.contains(&action.peer) {
                tree.initialize_peer(&action.peer, &tree.get_root_hash().unwrap())
                    .unwrap();
                initialized_peers.insert(action.peer.clone());
            }
            let header = TestHeader {
                hash: action.hash,
                slot: action.slot,
                parent: action.parent,
            };
            results.push(tree.select_roll_forward(&action.peer, header).unwrap());
        }
        results
    }

    /// Return a list of RollForward actions to execute for a given peer
    fn any_roll_forward_actions(
        depth: usize,
        max_length: usize,
    ) -> impl Strategy<Value=(HeadersTree<TestHeader>, Vec<RollForwardAction>)> {
        any_headers_tree(depth, max_length, 1).prop_flat_map(|tree| {
            // collect the tips of the tree
            let tips: Vec<NodeId> = tree
                .get_active_nodes()
                .map(|n| tree.arena.get_node_id(n).unwrap())
                .filter(|n| n.children(&tree.arena).count() == 0)
                .collect();

            // create a chain of headers for each tip (a path in the tree from the root to the tip)
            let mut chains: Vec<Vec<TestHeader>> = tips
                .into_iter()
                .map(|n| {
                    n.ancestors(&tree.arena)
                        .filter_map(|a| tree.arena.get(a).unwrap().get().to_header().cloned())
                        .collect()
                })
                .collect();
            chains.iter_mut().for_each(|c| c.reverse());

            // Associate a distinct peer to each chain and perform a topological sort
            // of all the roll forward actions necessary to create each chain
            let mut dag = Dag::new();
            let mut seen_nodes = BTreeSet::new();
            chains.into_iter().enumerate().for_each(|(i, chain)| {
                let peer = Peer::new(&format!("{}", i + 1));
                let mut parent_hash: Option<Hash<HEADER_HASH_SIZE>> = None;
                let mut parent_node: Option<RollForwardAction> = None;
                for h in chain.into_iter() {
                    let current_node = RollForwardAction {
                        peer: peer.clone(),
                        hash: h.hash,
                        slot: h.slot,
                        parent: parent_hash,
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
            let result: Vec<Vec<RollForwardAction>> = dag.toposort().unwrap_or(vec![]);

            // Randomly shuffle each level of the topological sort to simulate data coming from
            // peers concurrently and flatten the resulting list of actions.
            (
                Just(tree),
                Just(result)
                    .prop_flat_map(|r| shuffled_inner_vectors(r))
                    .prop_map(|vs| vs.into_iter().flatten().collect()),
            )
        })
    }

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

    proptest! {
        // Roundtrip check for CBOR encoding/decoding of TestHeaders
        #[test]
        fn prop_roundtrip_cbor(hdr in any_test_header()) {
            let bytes = to_cbor(&hdr);
            let hdr2 = from_cbor::<TestHeader>(&bytes).unwrap();
            assert_eq!(hdr, hdr2);
        }
    }

    /// HELPERS

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

    #[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord)]
    struct RollForwardAction {
        hash: Hash<HEADER_HASH_SIZE>,
        slot: u64,
        peer: Peer,
        parent: Option<Hash<HEADER_HASH_SIZE>>,
    }

    impl std::hash::Hash for RollForwardAction {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write(self.hash.as_slice())
        }
    }
}
