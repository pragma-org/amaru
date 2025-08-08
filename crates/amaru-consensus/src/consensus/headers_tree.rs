use crate::consensus::chain_selection::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::chain_selection::{Fork, ForwardChainSelection, RollbackChainSelection};
use crate::consensus::headers_tree::ChainAction::{
    ChainActionFork, ChainActionNewTip, ChainActionNoChange, ChainActionRollback,
};
use crate::consensus::tip::Tip;
use crate::peer::Peer;
use crate::ConsensusError;
use amaru_kernel::{Point, HEADER_HASH_SIZE, ORIGIN_HASH};
use amaru_ouroboros_traits::IsHeader;
use indextree::{Arena, Node, NodeId};
use pallas_crypto::hash::Hash;
use std::collections::{BTreeMap, HashSet};
use std::fmt::{Debug, Formatter};
use tracing::debug;

/// This data type stores chains as a tree of headers.
/// It also keeps track of what is the latest tip for each peer.
///
/// The main function of this data type is to be able to always return the best chain for the current
/// tree of headers.
///
#[allow(dead_code)]
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
            .unwrap_or("<UNKNOWN (WUT???)>".to_string());

        f.write_str(&self.pretty_print())?;
        f.write_str("\n")?;
        f.debug_struct("HeadersTree")
            .field("\n   peers", &debug_peers)
            .field("\n   best_chain", &debug_best_chain)
            .field(
                "\n   best_peer",
                &self.best_peer.as_ref().map(|p| p.to_string()),
            )
            .field("\n   max_length", &self.max_length)
            .finish()?;
        f.write_str("\n")
    }
}

#[derive(Clone, Debug, PartialOrd, PartialEq, Eq)]
enum ChainAction {
    ChainActionNewTip {
        tip: NodeId,
    },
    ChainActionNoChange,
    ChainActionRollback {
        rollback_node_id: NodeId,
    },
    ChainActionFork {
        peer: Peer,
        intersection_node_id: NodeId,
        best_tip: NodeId,
    },
}

#[allow(dead_code)]
impl<H: IsHeader + Clone + Debug> HeadersTree<H> {
    /// Initialize a HeadersTree as a tree rooted in the Genesis header
    pub fn new(max_length: usize, capacity: usize) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );

        // Create a new arena
        let mut arena: Arena<Tip<H>> = Arena::with_capacity(capacity);
        let genesis_node_id = arena.new_node(Tip::Genesis);
        HeadersTree {
            arena,
            max_length,
            peers: BTreeMap::new(),
            best_chain: genesis_node_id,
            best_peer: None,
        }
    }

    /// Initialize a HeadersTree with a list of headers
    pub fn initialize(headers: &Vec<H>, max_length: usize, capacity: usize) -> HeadersTree<H> {
        let mut tree = HeadersTree::new(max_length, capacity);
        tree.insert_headers(headers);
        tree
    }

    /// Add a peer and its current chain tip.
    /// This function must be invoked after the point header has been added to the tree,
    /// either during the initialization of the tree or a previous roll forward.
    pub fn initialize_peer(&mut self, peer: &Peer, point: &Point) -> Result<(), ConsensusError> {
        for node in self.arena.iter() {
            if node.get().hash() == Hash::from(point) {
                if let Some(node_id) = self.arena.get_node_id(node) {
                    self.peers.insert(peer.clone(), node_id);
                    if self.best_peer == None {
                        self.best_peer = Some(peer.clone())
                    }
                    return Ok(());
                }
            }
        }
        Err(ConsensusError::UnknownPoint(point.clone()))
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
            let e = ConsensusError::InvalidHeaderParent {
                peer: peer.clone(),
                forwarded: header.hash(),
                actual: header.parent().unwrap_or(ORIGIN_HASH),
                expected: self.get_hash(peer)?,
            };
            debug!("{e}. The current headers tree is {:?}", &self);
            return Err(e);
        };

        // Theres is nothing to do if the header has already been added
        if self.header_exists(&header) {
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
        rollback_point: &Point,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        // The peer must be known
        if !self.peers.contains_key(peer) {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        }

        let rollback_point_hash = rollback_point.hash();

        if let Some(rollback_node_id) = self.get_rollback_node_id(peer, &rollback_point_hash)? {
            self.select_best_chain_after_rollback(peer, &rollback_node_id)
        } else {
            let root = self
                .get_root_for(peer)?
                .map(|h| h.hash())
                .unwrap_or(Point::Origin.hash());
            Ok(RollbackBeyondLimit {
                peer: peer.clone(),
                rollback_point: rollback_point_hash,
                max_point: root,
            })
        }
    }
}

/// Main logic for determining the chain selection
impl<H: IsHeader + Clone + Debug> HeadersTree<H> {
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

        // Trim the best chain if too long
        self.prune_unreachable_nodes(&node_id);

        // 3 options:
        //
        // - The best chain was extended -> new tip
        // - Another shorter chain was extended but still shorter -> no change
        // - Another shorter chain was extended and now longer -> fork
        //
        let (new_best_tip, new_best_length) = (self.best_chain(), self.best_length());

        if self.get_parent(&new_best_tip) == Some(previous_best_tip) {
            Ok(ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip,
            })
        } else if new_best_length > previous_best_length {
            let fork = self.make_fork(&peer, previous_best_tip, self.best_chain);
            Ok(ForwardChainSelection::SwitchToFork(fork))
        } else {
            Ok(ForwardChainSelection::NoChange)
        }
    }

    fn select_best_chain_after_rollback(
        &mut self,
        peer: &Peer,
        rollback_node_id: &NodeId,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        let chain_action = self.select_chain_action(peer, rollback_node_id);
        self.peers.insert(peer.clone(), *rollback_node_id);
        self.trim_unused_nodes();
        self.update_best_chain(peer)?;

        let result = match chain_action {
            ChainActionNewTip { .. } => {
                // TODO return an error?
                RollbackChainSelection::NoChange
            }
            ChainActionNoChange => RollbackChainSelection::NoChange,
            ChainActionRollback { rollback_node_id } => {
                let hash = self.get_header(rollback_node_id).hash();
                RollbackChainSelection::RollbackTo(hash)
            }
            ChainActionFork {
                peer: best_peer,
                intersection_node_id,
                best_tip,
            } => {
                let fork = self.make_fork(&best_peer, intersection_node_id, best_tip);
                RollbackChainSelection::SwitchToFork(fork)
            }
        };
        Ok(result)
    }

    fn make_fork(&mut self, best_peer: &Peer, old_tip: NodeId, new_tip: NodeId) -> Fork<H> {
        let intersection_node_id = self.find_intersection_node_id(&old_tip, &new_tip);

        let forked_node_ids: Vec<NodeId> = new_tip
            .ancestors(&self.arena)
            .take_while(|n| *n != intersection_node_id)
            .collect();
        let mut fork_fragment: Vec<H> = forked_node_ids
            .iter()
            .filter_map(|n| {
                self.arena
                    .get(*n)
                    .and_then(|n| n.get().to_header().cloned())
            })
            .collect();
        fork_fragment.reverse();
        let fork = Fork {
            peer: best_peer.clone(),
            rollback_point: self
                .arena
                .get(intersection_node_id)
                .and_then(|n| n.get().to_header().map(|h| h.point()))
                .unwrap_or(Point::Origin),
            fork: fork_fragment,
        };
        fork
    }

    fn select_chain_action(&mut self, peer: &Peer, node_id: &NodeId) -> ChainAction {
        // If the current peer is the same as the previous best peer
        if Some(peer) == self.best_peer() || self.best_peer().is_none() {
            return self.select_own_chain_action(peer, &node_id);
        }

        // Otherwise, we are dealing with a different peer

        // If that user is contributing to the current best chain
        if let Some(parent) = self.get_parent(node_id) {
            if self
                .best_chain
                .ancestors(&self.arena)
                .collect::<Vec<_>>()
                .contains(&parent)
            {
                // If the peer new tip was added on top of the previous best tip
                // This is a new tip on the current best chain
                return if parent == self.best_chain() {
                    ChainActionNewTip { tip: *node_id }
                } else {
                    ChainActionNoChange
                };
            }
        }

        // Otherwise peer is contributing to its own chain
        // If the new chain becomes the best one it's a fork
        if node_id.ancestors(&self.arena).count() > self.best_length() {
            ChainActionFork {
                peer: peer.clone(),
                intersection_node_id: self.find_intersection_node_id(node_id, &self.best_chain()),
                best_tip: *node_id,
            }
        } else {
            ChainActionNoChange
        }
    }

    fn select_own_chain_action(&self, peer: &Peer, node_id: &NodeId) -> ChainAction {
        let current_node_id = *self
            .peers
            .get(peer)
            .unwrap_or_else(|| panic!("expected a peer {peer}"));

        // If the new tip was added on top of the previous chain tip
        if let Some(parent) = self.get_parent(node_id) {
            if parent == current_node_id {
                return ChainActionNewTip { tip: *node_id };
            }
        }

        let best_peer_so_far = self
            .peers
            .iter()
            .filter(|kv| kv.0 != peer)
            .map(|kv| (kv.0, kv.1, kv.1.ancestors(&self.arena).count()))
            .max_by_key(|kv| kv.1);

        if let Some((max_peer, max_node_id, max_length)) = best_peer_so_far {
            if node_id.ancestors(&self.arena).count() < max_length {
                ChainActionFork {
                    peer: max_peer.clone(),
                    intersection_node_id: self.find_intersection_node_id(node_id, max_node_id),
                    best_tip: *max_node_id,
                }
            } else {
                ChainActionRollback {
                    rollback_node_id: *node_id,
                }
            }
        } else {
            ChainActionRollback {
                rollback_node_id: *node_id,
            }
        }
    }
}

/// Helper functions on existing chains + nodes arena
impl<H: IsHeader + Clone + Debug> HeadersTree<H> {
    /// Return true if the parent of the header is the tip of the peer chain
    fn is_tip_child(&self, peer: &Peer, header: &H) -> Result<bool, ConsensusError> {
        let peer_current_hash = self.get_hash(peer)?;
        Ok(header.parent() == Some(peer_current_hash))
    }

    /// Return true if the header has already been added to the arena (and not rolled-back)
    fn header_exists(&self, header: &H) -> bool {
        let all_hashes: Vec<_> = self
            .arena
            .iter()
            .filter(|n| !n.is_removed())
            .map(|n| n.get().hash())
            .collect();
        all_hashes.contains(&header.hash())
    }

    /// Return the tip (NodeId) of a registered peer
    /// and return an error if the peer is not known.
    fn get_tip(&self, peer: &Peer) -> Result<NodeId, ConsensusError> {
        match self.peers.get(&peer) {
            Some(tip) => Ok(*tip),
            None => Err(ConsensusError::UnknownPeer(peer.clone())),
        }
    }

    /// Return the hash of the best header of a registered peer
    /// and return an error if the peer is not known.
    fn get_hash(&self, peer: &Peer) -> Result<Hash<HEADER_HASH_SIZE>, ConsensusError> {
        Ok(self.get_header(self.get_tip(peer)?).hash())
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
    fn update_best_chain(&mut self, peer: &Peer) -> Result<(), ConsensusError> {
        // If we try to set the best chain with a peer that is different from the previous one,
        // we only update the "best" data if the new peer chain is strictly longer than the previous one.
        if let Some(best_peer) = &self.best_peer {
            if best_peer == peer {
                self.best_chain = self.get_node_id_tip_for(best_peer)?;
            } else {
                let current_best_peer_chain_length = self.get_length(best_peer);
                let peer_chain_length = self.get_length(peer);
                if peer_chain_length > current_best_peer_chain_length {
                    self.best_chain = self.get_node_id_tip_for(peer)?;
                    self.best_peer = Some(peer.clone());
                }
            }
        } else {
            self.best_chain = self.get_node_id_tip_for(peer)?;
            self.best_peer = Some(peer.clone())
        };

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

    /// Return the chain tip for a given Peer
    fn get_node_id_tip_for(&self, peer: &Peer) -> Result<NodeId, ConsensusError> {
        Ok(*self
            .peers
            .get(peer)
            .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?)
    }

    /// Return the node id that is the least common parent between 2 node ids
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

    /// Return a header when it is expected to be found
    fn get_header(&self, node_id: NodeId) -> &H {
        self.unsafe_get_arena_node(node_id)
            .get()
            .to_header()
            .unwrap_or_else(|| panic!("expected to find a valid header for node id {node_id}"))
    }

    /// Insert headers into the arena and return the last created node id
    /// This function is used to initialize the tree from persistent storage.
    fn insert_headers(&mut self, headers: &Vec<H>) -> NodeId {
        let mut iter = headers.into_iter();
        if let Some(first) = iter.next() {
            let rest: Vec<_> = iter.collect();
            let mut last_node_id: NodeId = self.arena.new_node(Tip::Hdr(first.clone()));

            for header in rest {
                let new_node_id = self.arena.new_node(Tip::Hdr(header.clone()));
                last_node_id.append(new_node_id, &mut self.arena);
                last_node_id = new_node_id;
            }
            self.best_chain = last_node_id;
        };
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

        if let Some(parent_node_id) = self.get_parent(&node_id) {
            let ancestors = parent_node_id
                .ancestors(&self.arena)
                .collect::<Vec<NodeId>>();

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

                    // Make sure that no peer points to a removed node
                    let new_peers = self.peers.clone().into_iter().filter(|kv| !kv.1.is_removed(&self.arena)).collect::<Vec<_>>();
                    self.peers = BTreeMap::from_iter(new_peers);
                }
                _ => panic!("This chain selection tree is configured with a maximum chain length lower than 2 ({}), which does not make sense", self.max_length),
            }
        }
    }

    /// Walk through the list of all each peer tip
    /// and delete its descendants if they are not used in another chain
    fn trim_unused_nodes(&mut self) {
        let all_tips: HashSet<NodeId> = HashSet::from_iter(self.peers.values().map(|n| *n));
        trim_arena_unused_nodes(&mut self.arena, all_tips);
    }

    /// Pretty-print the headers present in the arena as a tree
    ///
    /// Shows a graphical representation of the internal structure of
    /// the tree.
    /// ```text
    /// Hdr(FakeHeader { block_number: 1, slot: 7, parent: None, body_hash: Hash<32>("2cabe6ea") })
    /// |-- Hdr(FakeHeader { block_number: 2, slot: 22, parent: Some(Hash<32>("d4f3cf2e")), body_hash: Hash<32>("cd932b1e") })
    /// |   `-- Hdr(FakeHeader { block_number: 3, slot: 23, parent: Some(Hash<32>("f72dbcd2")), body_hash: Hash<32>("5b466114") })
    /// |       `-- Hdr(FakeHeader { block_number: 4, slot: 26, parent: Some(Hash<32>("a0326a71")), body_hash: Hash<32>("993e8517") })
    /// |           `-- Hdr(FakeHeader { block_number: 5, slot: 28, parent: Some(Hash<32>("b452d00f")), body_hash: Hash<32>("9d341c29") })
    /// |               `-- Hdr(FakeHeader { block_number: 6, slot: 93, parent: Some(Hash<32>("143b3c68")), body_hash: Hash<32>("35894500") })
    /// |                   `-- Hdr(FakeHeader { block_number: 7, slot: 111, parent: Some(Hash<32>("448fa5c3")), body_hash: Hash<32>("b18964dc") })
    /// |                       `-- Hdr(FakeHeader { block_number: 8, slot: 115, parent: Some(Hash<32>("c3f7d827")), body_hash: Hash<32>("db0a9b64") })
    /// |                           `-- Hdr(FakeHeader { block_number: 9, slot: 124, parent: Some(Hash<32>("f1a8d8ce")), body_hash: Hash<32>("25a75c75") })
    /// |                               `-- Hdr(FakeHeader { block_number: 10, slot: 151, parent: Some(Hash<32>("17254ace")), body_hash: Hash<32>("9f3acd52") })
    /// `-- Hdr(FakeHeader { block_number: 2, slot: 0, parent: Some(Hash<32>("d4f3cf2e")), body_hash: Hash<32>("24115f11") })
    ///     `-- Hdr(FakeHeader { block_number: 3, slot: 0, parent: Some(Hash<32>("83ee63d6")), body_hash: Hash<32>("7950c684") })
    ///```
    fn pretty_print(&self) -> String {
        self.best_chain()
            .ancestors(&self.arena)
            .last()
            .map(|root| format!("{:?}", root.debug_pretty_print(&self.arena)))
            .unwrap_or("".to_string())
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

/// Walk through the list of all nodes that are younger than each tip
/// and delete them from the arena.
///
/// For example:
///
///  0 +- 1
///    +- 2 - 3 - 4
///    +- 5 - 6
///
///  peers = alice 2, bob 3, eve 5
///
/// Then, after trimming we get:
///
///  0 +- 1
///    +- 2 - 3
///
fn trim_arena_unused_nodes<T>(arena: &mut Arena<T>, all_tips: HashSet<NodeId>) {
    // list of nodes to remove with their descendants
    let mut to_remove: HashSet<NodeId> = HashSet::new();

    // find any tip that is not included in another chain
    // i.e has the tip of another chain as a descendant of its children
    for node in all_tips.iter() {
        for children in node.children(&arena) {
            let mut children_descendants = children.descendants(&arena);
            if !children_descendants.any(|n| all_tips.contains(&n)) {
                to_remove.insert(children);
            }
        }
    }
    for n in to_remove {
        n.remove_subtree(arena);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::chain_selection::tests::{generate_headers_anchored_at, random_bytes};
    use crate::consensus::chain_selection::ForwardChainSelection;
    use crate::consensus::chain_selection::RollbackChainSelection::RollbackTo;
    use crate::peer::Peer;
    use amaru_kernel::{Point, HEADER_HASH_SIZE};
    use amaru_ouroboros_traits::fake::FakeHeader;
    use test_macros::must_panic;

    #[test]
    fn empty() {
        let tree: HeadersTree<FakeHeader> = HeadersTree::new(10, 100);
        assert_eq!(
            tree.best_chain_tip(),
            None,
            "there is not best chain for an empty tree yet"
        );
    }

    #[test]
    fn single_chain_is_best_chain() {
        let headers = generate_headers_anchored_at(None, 5);
        let last = headers.last().unwrap();
        let mut tree = HeadersTree::new(10, 100);
        tree.insert_headers(&headers);

        assert_eq!(tree.best_chain_tip(), Some(last));
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn initialize_peer_on_known_point() {
        let (mut tree, headers) = create_headers_tree(5);

        let peer = Peer::new("alice");

        let last = headers.last().unwrap();
        let peer_point = Point::Specific(10, last.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        assert_eq!(tree.get_hash(&peer).unwrap(), last.hash());
    }

    #[test]
    fn initialize_peer_on_unknown_point_fails() {
        let mut tree = create_headers_tree(5).0;

        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, random_bytes(HEADER_HASH_SIZE));
        assert!(tree.initialize_peer(&peer, &peer_point).is_err());
    }

    #[test]
    fn roll_forward_extends_best_chain() {
        let (mut tree, mut headers) = create_headers_tree(5);
        let tip = headers.last().unwrap();

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

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
            headers.iter().collect::<Vec<&FakeHeader>>()
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
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward with the 6th block
        assert!(tree.select_roll_forward(&peer, new_tip).is_err());
    }

    #[test]
    fn roll_forward_from_unknown_peer_fails() {
        let (mut tree, headers) = create_headers_tree(5);
        let last = headers.last().unwrap();

        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, last.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

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
        tree.initialize_peer(&bob, &tip.point()).unwrap();

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
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn roll_forward_from_another_peer_on_a_smaller_chain_is_noop() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        // Initialize bob with the less headers than alice
        let bob = Peer::new("bob");
        let middle = headers[2];
        tree.initialize_peer(&bob, &middle.point()).unwrap();

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
            headers.iter().collect::<Vec<&FakeHeader>>(),
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
        tree.initialize_peer(&bob, &bob_tip.point()).unwrap();

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
            headers.iter().collect::<Vec<&FakeHeader>>(),
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
        tree.initialize_peer(&bob, &middle.point()).unwrap();
        let mut new_bob_headers = rollforward_from(&mut tree, &middle, &bob, 2);

        // Adding a new header must create a fork
        let bob_new_header3 = make_header_with_parent(&new_bob_headers.last().unwrap());
        new_bob_headers.push(bob_new_header3);
        let fork: Vec<FakeHeader> = new_bob_headers;
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
        tree.initialize_peer(&bob, &anchor.point()).unwrap();
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
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&alice, &peer_point).unwrap();

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
        let expected = expected.iter().collect::<Vec<&FakeHeader>>();
        assert_eq!(tree.best_chain_fragment(), expected);
    }

    #[test]
    fn roll_forward_with_one_chain_an_unreachable_anchor_must_be_dropped() {
        // create a chain at max length
        let (mut tree, headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&alice, &peer_point).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        _ = tree.select_roll_forward(&alice, new_tip).unwrap();

        // The original chain anchor is still present in the arena but marked as removed (its data is dropped)
        assert_eq!(tree.size(), 12);

        // If we add a new header to the current chain, the arena reuses the removed node to store
        // the new header
        let new_tip = make_header_with_parent(&new_tip.clone());
        _ = tree.select_roll_forward(&alice, new_tip).unwrap();
        assert_eq!(tree.size(), 12);
    }

    #[test]
    fn roll_forward_beyond_limit_drops_unreachable_chain() {
        // create a chain at max length
        let (mut tree, headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&alice, &peer_point).unwrap();

        let bob = Peer::new("bob");
        let first = headers.first().unwrap();
        tree.initialize_peer(&bob, &first.point()).unwrap();

        // Roll forward with 2 new headers from bob
        _ = rollforward_from(&mut tree, first, &bob, 2);

        // Now roll forward alice's tip twice
        // The tree must not grow in size because bob's arena nodes have been reused
        _ = rollforward_from(&mut tree, tip, &alice, 4);
        assert_eq!(tree.size(), 14);
    }

    #[test]
    fn rollback_on_the_best_chain() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);
        let middle = headers[3];
        let result = RollbackTo(middle.hash());
        assert_eq!(
            tree.select_rollback(&alice, &middle.point()).unwrap(),
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
        tree.select_rollback(&alice, &rollback_to.point()).unwrap();

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
        let result = tree.select_rollback(&alice, &middle.point()).unwrap();
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
        tree.initialize_peer(&bob, &headers[1].point()).unwrap();

        // Bob tries to rollback on a header that's not part of its chain
        let result = tree.select_rollback(&bob, &headers[3].point()).unwrap();
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
            tree.select_rollback(&alice, &rollback_point.point())
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
            tree.select_rollback(&alice, &rollback_point.point())
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
        let header1 = headers[1].clone();
        tree.initialize_peer(&bob, &header1.point()).unwrap();
        let added_headers = rollforward_from(&mut tree, &header1, &bob, 4); // 4 added headers

        // sanity check: bob chain is the longest
        assert_eq!(
            tree.best_chain_tip().unwrap(),
            added_headers.last().unwrap()
        );

        // Now bob is rolled back 2 headers
        let result = tree
            .select_rollback(&bob, &added_headers[1].point())
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
    #[must_panic(expected = "Cannot create a headers tree with maximum chain length lower than 2")]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<FakeHeader>::new(1, 1000);
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

        let peer_tips = HashSet::from_iter(vec![n_0, n_1, n_2, n_2_1, n_2_2]);
        trim_arena_unused_nodes(&mut arena, peer_tips);

        let active_nodes: HashSet<&str> = arena
            .iter()
            .filter(|n| !n.is_removed())
            .map(|n| *n.get())
            .collect();
        assert_eq!(
            active_nodes,
            HashSet::from_iter(vec!["0", "1", "2", "2_1", "2_2"])
        )
    }

    /// HELPERS
    fn create_headers_tree(size: u32) -> (HeadersTree<FakeHeader>, Vec<FakeHeader>) {
        let mut tree = HeadersTree::new(10, 100);
        let headers = generate_headers_anchored_at(None, size);
        _ = tree.insert_headers(&headers);
        (tree, headers)
    }

    fn initialize_with_peer(size: u32, peer: &Peer) -> (HeadersTree<FakeHeader>, Vec<FakeHeader>) {
        let (mut tree, headers) = create_headers_tree(size);
        let tip = headers.last().unwrap();
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(peer, &peer_point).unwrap();
        (tree, headers)
    }

    fn make_header_with_parent(parent: &FakeHeader) -> FakeHeader {
        FakeHeader {
            block_number: parent.block_number + 1,
            slot: 0,
            parent: Some(parent.hash()),
            body_hash: random_bytes(HEADER_HASH_SIZE).as_slice().into(),
        }
    }

    /// Roll forward the tree:
    ///  - Starting from a given header,
    ///  - For a given peer
    ///  - A number of times
    ///
    /// And return the last created header
    fn rollforward_from(
        tree: &mut HeadersTree<FakeHeader>,
        header: &FakeHeader,
        peer: &Peer,
        times: u32,
    ) -> Vec<FakeHeader> {
        let mut result = vec![];
        let mut parent = header.clone();
        for _ in 0..times {
            let next_header = make_header_with_parent(&parent);
            tree.select_roll_forward(peer, next_header).unwrap();
            result.push(next_header);
            parent = next_header.clone();
        }
        result
    }
}
