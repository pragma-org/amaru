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

use crate::consensus::errors::ConsensusError::UnknownPoint;
use crate::consensus::errors::{ConsensusError, InvalidHeaderParentData};
use crate::consensus::headers_tree::HeadersTreeDisplay;
use crate::consensus::headers_tree::headers_tree::Tracker::{Me, SomePeer};
use crate::consensus::headers_tree::tree::Tree;
use crate::consensus::stages::select_chain::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::stages::select_chain::{Fork, ForwardChainSelection, RollbackChainSelection};
use amaru_kernel::IsHeader;
use amaru_kernel::string_utils::ListToString;
use amaru_kernel::{HeaderHash, ORIGIN_HASH, Point, peer::Peer};
use amaru_ouroboros_traits::ChainStore;
#[cfg(any(test, feature = "test-utils"))]
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use tracing::instrument;

/// This data type stores the chains of headers known from different peers:
///
///  - max_length: the maximum length allowed for a chain. This is used to prune old headers.
///    In production this is set to the consensus_security_parameter.
///  - headers: a map of all the known headers by their hash.
///  - tree: a tree representation of the headers to be able to traverse from root to leaves.
///  - peers: a map of the latest tip for each peer.
///
#[derive(Clone)]
pub struct HeadersTree<H> {
    /// Transient state of the headers tree with peers and max length
    tree_state: HeadersTreeState,
    /// Persistent state of the headers tree within the chain store
    chain_store: Arc<dyn ChainStore<H>>,
}

impl<H> HeadersTree<H> {
    /// Consume the headers tree and return its transient state
    pub fn into_tree_state(self) -> HeadersTreeState {
        self.tree_state
    }
}

/// Transient state of the headers tree with peers and max length
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Default)]
pub struct HeadersTreeState {
    max_length: usize,
    peers: BTreeMap<Tracker, Vec<HeaderHash>>,
}

impl Display for HeadersTreeState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "  peers: {}",
            &self
                .peers
                .iter()
                .map(|(p, hs)| format!("{} -> [{}]", p, hs.list_to_string(", ")))
                .join(", ")
        )?;
        writeln!(f, "  max_length: {}", &self.max_length)?;
        Ok(())
    }
}

impl HeadersTreeState {
    pub fn new(max_length: usize) -> Self {
        Self {
            max_length,
            peers: BTreeMap::new(),
        }
    }

    /// Return the peers and their current chain tips for testing.
    #[cfg(test)]
    pub(crate) fn peers(&self) -> &BTreeMap<Tracker, Vec<HeaderHash>> {
        &self.peers
    }

    /// Add a peer and its current chain tip to initialize the current headers tree state.
    pub fn initialize_peer<H: IsHeader + Clone + 'static>(
        &mut self,
        store: Arc<dyn ChainStore<H>>,
        peer: &Peer,
        hash: &HeaderHash,
    ) -> Result<(), ConsensusError> {
        if store.has_header(hash) || hash == &ORIGIN_HASH {
            let mut peer_chain: Vec<_> = store.ancestors_hashes(hash).collect();
            peer_chain.reverse();
            self.peers.insert(SomePeer(peer.clone()), peer_chain);
            Ok(())
        } else {
            Err(UnknownPoint(*hash))
        }
    }
}

/// A tracker is either "Me" (the local node) or a specific peer.
/// We use Me to keep track of the best known chain, either when there are no peers yet,
/// or when the best chain is being switched to a fork. In that case we want to wait until we
/// have built the new best chain locally before we send Rollback + RollForward events downstream.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub(crate) enum Tracker {
    #[default]
    Me,
    SomePeer(Peer),
}

impl Tracker {
    /// Return the peer if this tracker is not "Me"
    fn to_peer(&self) -> Option<Peer> {
        match self {
            Me => None,
            SomePeer(p) => Some(p.clone()),
        }
    }
}

impl Display for Tracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Me => f.write_str("Me"),
            SomePeer(p) => f.write_str(&p.to_string()),
        }
    }
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq + 'static> Debug for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.format(f, |tip| format!("{tip:?}"))
    }
}

impl<H: IsHeader + Debug + Clone + Display + PartialEq + Eq + 'static> Display for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.format(f, |tip| format!("{tip}"))
    }
}

impl<H: IsHeader + Debug + Clone + PartialEq + Eq + 'static> HeadersTree<H> {
    fn format(
        &self,
        f: &mut Formatter<'_>,
        header_to_string: fn(&H) -> String,
    ) -> std::fmt::Result {
        let tree_display = HeadersTreeDisplay::from(self.clone());
        tree_display.format(f, header_to_string)
    }

    /// Return the tree representation of the headers tree.
    pub fn to_tree(&self) -> Option<Tree<H>> {
        let mut as_map = BTreeMap::new();
        for header in self.load_headers(&self.anchor()) {
            let _ = as_map.insert(header.hash(), header);
        }
        Tree::from(&as_map)
    }

    /// Load all the headers in the subtree rooted at the given hash
    fn load_headers(&self, root: &HeaderHash) -> Vec<H> {
        let mut headers = vec![];
        if let Some(header) = self.chain_store.load_header(root) {
            headers.push(header);
        }

        for hash in self.chain_store.get_children(root) {
            headers.extend(self.load_headers(&hash));
        }
        headers
    }
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq + Send + Sync + 'static> HeadersTree<H> {
    /// Create a new HeadersTree with a given store and maximum chain length.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new(store: Arc<dyn ChainStore<H>>, max_length: usize) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );
        let mut peers = BTreeMap::new();
        peers.insert(Me, store.retrieve_best_chain());
        let tree_state = HeadersTreeState { max_length, peers };
        HeadersTree::create(store, tree_state)
    }

    /// Create a new HeadersTree with an in-memory store for testing purposes.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_in_memory(max_length: usize) -> HeadersTree<H> {
        HeadersTree::new(Arc::new(InMemConsensusStore::new()), max_length)
    }

    /// Create a new HeadersTree
    pub(crate) fn create(
        chain_store: Arc<dyn ChainStore<H>>,
        tree_state: HeadersTreeState,
    ) -> HeadersTree<H> {
        HeadersTree {
            tree_state,
            chain_store,
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
        if self.chain_store.has_header(hash) || hash == &ORIGIN_HASH {
            let mut peer_chain: Vec<_> = self.ancestors_hashes(hash).collect();
            peer_chain.reverse();
            self.tree_state
                .peers
                .insert(SomePeer(peer.clone()), peer_chain);
            Ok(())
        } else {
            Err(UnknownPoint(*hash))
        }
    }

    /// Add a new header coming from an upstream peer to the tree.
    /// The result will be an event describing:
    ///   - No change (if the header is already known for example).
    ///   - A NewTip.
    ///   - A Fork if the chain that ends with this new header becomes the longest one.
    ///
    /// There will be errors if:
    ///   - The header's parent is unknown
    ///
    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        tip: &H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        // The header must be a child of the peer's tip
        if !self.is_tip_child(peer, tip) {
            let e = ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer.clone(),
                forwarded: tip.point(),
                actual: tip.parent(),
                expected: self.get_point(peer)?,
            }));
            return Err(e);
        };

        let result = if self.is_empty_tree() {
            // special case for the first header added to an empty tree
            self.update_peer(peer, tip);
            if self.anchor() == ORIGIN_HASH {
                self.set_anchor(&tip.hash())?;
            }
            self.set_best_chain(&tip.hash())?;
            Ok(ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip: tip.clone(),
            })
        } else if self.extends_best_chains(tip) {
            self.update_peer(peer, tip);
            // If the tip is extending _the_ best chain
            let result = if tip.parent().as_ref() == Some(&self.best_chain()) {
                Ok(ForwardChainSelection::NewTip {
                    peer: peer.clone(),
                    tip: tip.clone(),
                })
            } else {
                let fork = self.make_fork(peer, &self.best_chain(), &tip.hash());
                Ok(ForwardChainSelection::SwitchToFork(fork))
            };
            self.set_best_chain(&tip.hash())?;
            result
        } else {
            self.update_peer(peer, tip);
            Ok(ForwardChainSelection::NoChange)
        };

        self.trim_chain()?;
        result
    }

    fn extends_best_chains(&mut self, tip: &H) -> bool {
        self.best_chains()
            .iter()
            .any(|h| Some(*h) == tip.parent().as_ref())
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
        // The peer tip must be known
        if let Some(peer_tip) = self.get_peer_tip(peer).cloned() {
            // This is a no-op but possibly we need to flag the peer as adversarial
            if &peer_tip == rollback_hash {
                return Ok(RollbackChainSelection::NoChange);
            }

            // Remove invalid headers for that peer
            // If the rollback hash is not found indicate that the rollback point must be too far in the past.
            if self.rollback_peer_chain(peer, rollback_hash).is_err() {
                return Ok(RollbackBeyondLimit {
                    peer: peer.clone(),
                    rollback_point: *rollback_hash,
                    max_point: self.anchor(),
                });
            };

            if self.best_chain() == peer_tip {
                // recompute the best chains
                let best_chains = self.best_chains();

                // Otherwise we switch to a better chain -> Switch to fork
                let best = if let Some(first) = best_chains.first() {
                    **first
                } else {
                    self.best_chain()
                };
                self.set_best_chain(&best)?;
                if let Some(best_peer) = self.best_peer() {
                    let fork = self.make_fork(&best_peer, rollback_hash, &self.best_chain());
                    Ok(RollbackChainSelection::SwitchToFork(fork))
                } else {
                    Ok(RollbackChainSelection::NoChange)
                }
            } else {
                // Otherwise the change did not affect the best chain -> NoChange
                Ok(RollbackChainSelection::NoChange)
            }
        } else {
            Err(ConsensusError::UnknownPeer(peer.clone()))
        }
    }
}

impl<H: IsHeader + Clone + Debug + 'static + PartialEq + Eq> HeadersTree<H> {
    /// Return the length of the best chain currently known.
    pub fn best_length(&self) -> usize {
        self.tree_state
            .peers
            .values()
            .map(|chain| chain.len())
            .max()
            .unwrap_or_default()
    }

    /// Return the tracker of the best chain, or "Me" if no peer is tracking it.
    fn best_tracker(&self) -> &Tracker {
        let best_chain = self.best_chain();
        self.tree_state
            .peers
            .iter()
            .find(|(p, chain)| p != &&Me && chain.last() == Some(&best_chain))
            .map(|pc| pc.0)
            .unwrap_or(&Me)
    }

    /// Return the peer tracking the best chain, or None if "Me" is tracking it.
    fn best_peer(&self) -> Option<Peer> {
        self.best_tracker().to_peer()
    }

    /// Return the list of the best chains currently known.
    pub fn best_chains(&self) -> Vec<&HeaderHash> {
        let mut best_length = 0;
        let mut best_chains = vec![];
        for chain in self.tree_state.peers.values() {
            if let Some(last) = chain.last() {
                if chain.len() > best_length {
                    best_length = chain.len();
                    best_chains = vec![last];
                } else if chain.len() == best_length && !best_chains.contains(&last) {
                    best_chains.push(last);
                }
            }
        }
        best_chains.sort_by_key(|h| self.unsafe_get_header(h).slot());
        best_chains
    }

    /// Return true if the tree is empty, i.e. it only contains the origin header.
    fn is_empty_tree(&self) -> bool {
        self.anchor() == ORIGIN_HASH
    }

    /// Return the root of the tree
    pub fn anchor(&self) -> HeaderHash {
        self.chain_store.get_anchor_hash()
    }

    /// Remove all the header hashes for a peer chain after (and excluding) the given hash.
    /// Return an error if the hash is not found in the peer chain.
    fn rollback_peer_chain(
        &mut self,
        peer: &Peer,
        hash: &HeaderHash,
    ) -> Result<(), ConsensusError> {
        if let Some(chain) = self.tree_state.peers.get_mut(&SomePeer(peer.clone())) {
            if let Some(rollback_index) = chain.iter().position(|h| h == hash) {
                // keep everything up to and including the rollback hash
                chain.truncate(rollback_index + 1);
            } else {
                return Err(UnknownPoint(*hash));
            }
        };
        Ok(())
    }

    /// Create a fork for a given peer:
    ///  - the old tip is the tip of the previous best chain
    ///  - the new tip is the tip of the new best chain
    fn make_fork(&self, best_peer: &Peer, old_tip: &HeaderHash, new_tip: &HeaderHash) -> Fork<H> {
        let intersection_hash = self.find_intersection_hash(old_tip, new_tip);

        // get all the hashes between the new tip and the forking hash
        let mut fork_fragment: Vec<H> = vec![];
        let mut current = self.unsafe_get_header(new_tip);
        while current.hash() != intersection_hash {
            if let Some(parent) = current.parent() {
                fork_fragment.push(current.clone());
                current = self.unsafe_get_header(&parent);
            } else {
                break;
            }
        }
        fork_fragment.reverse();

        Fork {
            peer: best_peer.clone(),
            rollback_header: self.unsafe_get_header(&intersection_hash),
            fork: fork_fragment,
        }
    }

    /// Update the peer chain with the new tip
    fn update_peer(&mut self, peer: &Peer, tip: &H) {
        let tracker = SomePeer(peer.clone());
        if let Some(chain) = self.tree_state.peers.get_mut(&tracker) {
            chain.push(tip.hash());
        } else {
            self.tree_state.peers.insert(tracker, vec![tip.hash()]);
        }
    }

    /// Return the header for a given hash, panicking if it does not exist.
    /// This function should only with precaution, when we are sure the header exists.
    #[expect(clippy::panic)]
    fn unsafe_get_header(&self, hash: &HeaderHash) -> H {
        self.get_header(hash)
            .unwrap_or_else(|| panic!("A header must exist for hash {}", hash))
    }

    /// Return the header for a given hash, or None if it does not exist.
    fn get_header(&self, hash: &HeaderHash) -> Option<H> {
        self.chain_store.load_header(hash)
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors_hashes(&self, hash: &HeaderHash) -> Box<dyn Iterator<Item = HeaderHash> + '_> {
        self.chain_store.ancestors_hashes(hash)
    }

    /// Return true if the parent of the header is the tip of the peer chain
    ///
    /// We need to special case for the discrepancy between:
    ///
    ///  - A Header having no parent (i.e. the origin header) or a parent set to the ORIGIN_HASH.
    ///  - The ORIGIN_HASH being used as the default root hash value in the tree.
    ///
    fn is_tip_child(&self, peer: &Peer, header: &H) -> bool {
        match self.get_peer_tip(peer) {
            Some(peer_hash) => {
                if let Some(parent_hash) = header.parent() {
                    peer_hash == &parent_hash
                } else {
                    peer_hash == &ORIGIN_HASH
                }
            }
            None => header.parent().is_none() || header.parent() == Some(ORIGIN_HASH),
        }
    }

    /// Return the tip of the chain for a given peer, or None if the peer is unknown.
    fn get_peer_tip(&self, peer: &Peer) -> Option<&HeaderHash> {
        self.tree_state
            .peers
            .get(&SomePeer(peer.clone()))
            .and_then(|hs| hs.last())
    }

    /// Return the best currently known tip
    pub(crate) fn best_chain(&self) -> HeaderHash {
        self.chain_store.get_best_chain_hash()
    }

    /// Store the best currently known tip and update our tracker to the new best chain fragment.
    fn set_best_chain(&mut self, hash: &HeaderHash) -> Result<(), ConsensusError> {
        self.chain_store
            .set_best_chain_hash(hash)
            .map_err(|e| ConsensusError::SetBestChainHashFailed(*hash, e))?;
        // update "Me" so that it points to the best chain, as given by the the best chain tip
        self.tree_state
            .peers
            .insert(Me, self.best_chain_fragment_hashes());
        Ok(())
    }

    /// Store the current chain anchor if it has changed.
    fn set_anchor(&mut self, hash: &HeaderHash) -> Result<(), ConsensusError> {
        self.chain_store
            .set_anchor_hash(hash)
            .map_err(|e| ConsensusError::SetAnchorHashFailed(*hash, e))?;
        Ok(())
    }

    /// Return the hash of the best header of a registered peer
    /// and return an error if the peer is not known.
    fn get_point(&self, peer: &Peer) -> Result<Point, ConsensusError> {
        let tip = self
            .get_peer_tip(peer)
            .ok_or(ConsensusError::UnknownPeer(peer.clone()))?;
        Ok(self
            .chain_store
            .load_header(tip)
            .map(|t| t.point())
            .unwrap_or(Point::Origin))
    }

    /// Return the header hash that is the least common parent between 2 headers in the tree
    /// This implementation avoids materializing the full set of ancestors for both headers.
    fn find_intersection_hash(&self, hash1: &HeaderHash, hash2: &HeaderHash) -> HeaderHash {
        if hash1 == hash2 {
            return *hash1;
        }
        let ancestors1 = self.ancestors_hashes(hash1);
        let mut ancestors2 = self.ancestors_hashes(hash2);
        let mut seen = BTreeSet::new();
        for h1 in ancestors1 {
            if seen.contains(&h1) {
                return h1;
            }
            for h2 in ancestors2.by_ref() {
                if h1 == h2 {
                    return h1;
                }
                seen.insert(h2);
            }
        }
        self.anchor()
    }

    /// When the best chain exceeds the maximum length, move its root up one level and
    /// remove the peers that do not contain the new anchor.
    #[instrument(level = "trace", skip_all)]
    fn trim_chain(&mut self) -> Result<(), ConsensusError> {
        if self.best_length() <= self.tree_state.max_length {
            return Ok(());
        }

        // Get the first two headers of the best chain fragment
        let second = {
            let mut best_chain_fragment = self.best_chain_fragment_hashes_iterator();
            best_chain_fragment.next();
            best_chain_fragment.next()
        };

        // If we have at least two headers, we can move the anchor up one level
        // We remove all the peers which do not contain the new anchor
        if let Some(second) = second {
            self.tree_state.peers.iter_mut().for_each(|(_p, hs)| {
                if !hs.contains(&second) {
                    hs.drain(0..);
                } else {
                    hs.remove(0);
                }
            });
            self.tree_state.peers.retain(|_p, hs| !hs.is_empty());
            self.set_anchor(&second)?;
            Ok(())
        } else {
            Ok(())
        }
    }

    /// Return the best chain fragment currently known as a list of hashes.
    /// The list starts from the root.
    fn best_chain_fragment_hashes(&self) -> Vec<HeaderHash> {
        let best_chain = self.best_chain();
        self.tree_state
            .peers
            .values()
            .find(|chain| chain.last() == Some(&best_chain))
            .cloned()
            .unwrap_or_default()
    }

    /// Return the best chain fragment currently known as a list of hashes.
    /// The list starts from the root.
    fn best_chain_fragment_hashes_iterator(&self) -> impl Iterator<Item = HeaderHash> {
        let best_chain = self.best_chain();
        self.tree_state
            .peers
            .values()
            .find(|chain| chain.last() == Some(&best_chain))
            .map(|chain| chain.iter().cloned())
            .unwrap_or_default()
    }
}

#[cfg(any(test, doc, feature = "test-utils"))]
/// Those functions are only used by tests
impl<H: IsHeader + Clone + Debug + PartialEq + Eq + Send + Sync + 'static> HeadersTree<H> {
    /// Return true if the peer is known
    pub fn has_peer(&self, peer: &Peer) -> bool {
        self.tree_state.peers.contains_key(&SomePeer(peer.clone()))
    }

    /// Return the tip of the best chain that currently known as a header
    pub fn best_chain_tip(&self) -> H {
        self.unsafe_get_header(&self.best_chain())
    }

    /// Return the best chain fragment currently known as a list of headers.
    /// The list starts from the root.
    pub fn best_chain_fragment(&self) -> Vec<H> {
        let mut fragment: Vec<_> = self
            .chain_store
            .ancestors(self.unsafe_get_header(&self.best_chain()))
            .collect();
        fragment.reverse();
        fragment
    }

    /// Return the headers tree size in terms of how many headers are being tracked.
    /// This is used to check the garbage collection aspect of this data structure.
    #[cfg(test)]
    fn size(&self) -> usize {
        self.load_headers(&self.anchor()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::headers_tree::data_generation::SelectionResult::Forward;
    use crate::consensus::headers_tree::data_generation::*;
    use crate::consensus::stages::select_chain::Fork;
    use crate::consensus::stages::select_chain::ForwardChainSelection::SwitchToFork;
    use amaru_kernel::BlockHeader;
    use amaru_kernel::string_utils::{ListDebug, ListsToString};
    use amaru_kernel::tests::random_hash;
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use proptest::proptest;
    use std::assert_matches::assert_matches;

    #[test]
    fn initialize_peer_on_empty_tree() {
        let store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::new());
        let peer = Peer::new("alice");
        let header = generate_single_header();
        store.store_header(&header).unwrap();
        let mut tree: HeadersTree<BlockHeader> =
            HeadersTree::create(store, HeadersTreeState::new(10));
        tree.initialize_peer(&peer, &Point::Origin.hash()).unwrap();

        tree.select_roll_forward(&peer, &header).unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            header,
            "there must be a best chain available after the first roll forward"
        );
    }

    #[test]
    fn single_chain_is_best_chain() {
        let headers = generate_headers_chain(5);
        let store = Arc::new(InMemConsensusStore::new());
        let tree: HeadersTree<BlockHeader> =
            HeadersTree::create(store.clone(), HeadersTreeState::new(10));
        for header in &headers {
            store.store_header(header).unwrap();
        }
        store.set_anchor_hash(&headers[0].hash()).unwrap();
        store.set_best_chain_hash(&headers[4].hash()).unwrap();

        assert_eq!(tree.size(), 5);
        assert_eq!(tree.best_chain_tip(), headers[4]);
        assert_eq!(tree.best_chain_fragment(), headers);
    }

    #[test]
    fn initialize_peer_on_known_point() {
        let mut tree = create_headers_tree(5);
        let headers = tree.best_chain_fragment();
        let peer = Peer::new("alice");

        let last_hash = headers[4].hash();
        tree.initialize_peer(&peer, &last_hash).unwrap();

        assert_eq!(
            tree.get_point(&peer).unwrap(),
            headers[4].point(),
            "last_hash {last_hash}\ntree {tree}"
        );
    }

    #[test]
    fn initialize_peer_on_unknown_point_fails() {
        let mut tree = create_headers_tree(5);

        let peer = Peer::new("alice");
        assert!(tree.initialize_peer(&peer, &random_hash()).is_err());
    }

    #[test]
    fn roll_forward_extends_best_chain() {
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = create_headers_tree_with_store(store.clone(), 5);
        let mut headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();
        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &tip.hash()).unwrap();

        // Now roll forward extending tip
        let new_tip = store_header_with_parent(store.clone(), tip);
        let result = tree.select_roll_forward(&peer, &new_tip).unwrap();
        assert_eq!(
            result,
            ForwardChainSelection::NewTip {
                peer,
                tip: new_tip.clone()
            }
        );
        assert_eq!(tree.best_chain_tip(), new_tip);

        headers.push(new_tip);
        let best_chain_fragment = tree.best_chain_fragment();
        assert_eq!(
            best_chain_fragment,
            headers,
            "\nactual\n{}\n\nexpected\n{}\n",
            best_chain_fragment.list_to_string(",\n"),
            headers.list_to_string(",\n")
        );
    }

    #[test]
    fn roll_forward_with_incorrect_parent_fails() {
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = create_headers_tree_with_store(store.clone(), 5);
        let headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        // create a new tip pointing to an incorrect parent (the first header of the chain in this example)
        let new_tip = store_header_with_parent(store.clone(), headers.first().unwrap());

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &tip.hash()).unwrap();

        // Now roll forward with the 6th block
        assert!(tree.select_roll_forward(&peer, &new_tip).is_err());
    }

    #[test]
    fn roll_forward_from_unknown_peer_fails() {
        let mut tree = create_headers_tree(5);
        let headers = tree.best_chain_fragment();
        let last = headers.last().unwrap().clone();

        let peer = Peer::new("alice");
        tree.initialize_peer(&peer, &last.hash()).unwrap();

        // Now roll forward with an unknown peer
        let peer = Peer::new("bob");
        assert!(tree.select_roll_forward(&peer, &last).is_err());
    }

    #[test]
    fn roll_forward_from_another_peer_at_tip_extends_best_chain() {
        let alice = Peer::new("alice");
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = initialize_with_store_and_peer(store.clone(), 5, &alice);
        let mut headers = tree.best_chain_fragment();

        // Initialize bob with the same headers as alice
        let bob = Peer::new("bob");
        let tip = headers.last().unwrap();
        let new_tip = store_header_with_parent(store, tip);
        tree.initialize_peer(&bob, &tip.hash()).unwrap();

        // Roll forward with a new header from bob, on the same chain
        assert_eq!(
            tree.select_roll_forward(&bob, &new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: bob,
                tip: new_tip.clone()
            }
        );
        assert_eq!(tree.best_chain_tip(), new_tip);

        headers.push(new_tip);
        assert_eq!(tree.best_chain_fragment(), headers);
    }

    #[test]
    fn roll_forward_from_another_peer_on_a_smaller_chain_is_noop() {
        let alice = Peer::new("alice");
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = initialize_with_store_and_peer(store.clone(), 5, &alice);
        let headers = tree.best_chain_fragment();

        // Initialize bob with the less headers than alice
        let bob = Peer::new("bob");
        let middle = &headers[2];
        tree.initialize_peer(&bob, &middle.hash()).unwrap();

        // Roll forward with a new header from bob
        let new_tip_for_bob = store_header_with_parent(store, middle);
        assert_eq!(
            tree.select_roll_forward(&bob, &new_tip_for_bob).unwrap(),
            ForwardChainSelection::NoChange
        );
        let tip = headers.last().unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            *tip,
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
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = initialize_with_store_and_peer(store.clone(), 5, &alice);
        let headers = tree.best_chain_fragment();

        // Initialize bob with same chain than alice minus the last header
        let bob = Peer::new("bob");
        let bob_tip = &headers[3];
        tree.initialize_peer(&bob, &bob_tip.hash()).unwrap();

        // Roll forward with a new header from bob
        let new_tip_for_bob = store_header_with_parent(store, bob_tip);
        assert_eq!(
            tree.select_roll_forward(&bob, &new_tip_for_bob).unwrap(),
            ForwardChainSelection::NoChange
        );
        let tip = headers.last().unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            *tip,
            "the current tip must not change"
        );

        assert_eq!(
            tree.best_chain_fragment().list_to_string(",\n"),
            headers.list_to_string(",\n"),
            "the best chain hasn't changed"
        );
    }

    #[test]
    fn roll_forward_from_another_peer_creates_a_fork_if_the_new_chain_is_the_best() {
        let alice = Peer::new("alice");
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = initialize_with_store_and_peer(store.clone(), 5, &alice);
        let headers = tree.best_chain_fragment();

        // Initialize bob with some headers common with alice + additional headers that are different
        // so that their chains have the same length
        let bob = Peer::new("bob");
        let middle = headers[2].clone();
        tree.initialize_peer(&bob, &middle.hash()).unwrap();
        let mut new_bob_headers = rollforward_from(&mut tree, &middle, &bob, 2);

        // Adding a new header must create a fork
        let bob_new_header3 = store_header_with_parent(store, new_bob_headers.last().unwrap());
        new_bob_headers.push(bob_new_header3.clone());
        let fork: Vec<BlockHeader> = new_bob_headers;
        let fork = Fork {
            peer: bob.clone(),
            rollback_header: middle,
            fork,
        };

        let result = tree.select_roll_forward(&bob, &bob_new_header3).unwrap();
        assert_eq!(result, SwitchToFork(fork));
    }

    #[test]
    fn roll_forward_with_fork_to_a_disjoint_chain() {
        let alice = Peer::new("alice");
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = initialize_with_store_and_peer(store.clone(), 5, &alice);

        let headers = tree.best_chain_fragment();
        let anchor = headers.first().unwrap();

        // Initialize bob with a completely different chain of the same size
        let bob = Peer::new("bob");
        tree.initialize_peer(&bob, &anchor.hash()).unwrap();
        let mut bob_headers = rollforward_from(&mut tree, anchor, &bob, 4);
        let bob_tip = bob_headers.last().unwrap();

        // Adding a new header for bob must create a fork
        let bob_new_tip = store_header_with_parent(store, bob_tip);
        bob_headers.push(bob_new_tip.clone());
        let fork = Fork {
            peer: bob.clone(),
            rollback_header: anchor.clone(),
            fork: bob_headers,
        };

        let result = tree.select_roll_forward(&bob, &bob_new_tip).unwrap();
        assert_eq!(result, SwitchToFork(fork));
    }

    #[test]
    fn roll_forward_beyond_max_length() {
        // create a chain at max length
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = create_headers_tree_with_store(store.clone(), 10);
        let mut headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        // Now roll forward extending tip
        let new_tip = store_header_with_parent(store, tip);
        assert_eq!(
            tree.select_roll_forward(&alice, &new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: new_tip.clone()
            }
        );
        assert_eq!(tree.best_chain_tip(), new_tip);

        // The expected chain ends with the new tip but starts with the second header
        // in order to stay below max_length
        headers.push(new_tip.clone());
        let expected = headers.split_off(1);
        assert_eq!(
            tree.best_chain_fragment(),
            expected,
            "actual\n{}\n\nexpected\n{}\n",
            tree.best_chain_fragment().list_debug(",\n"),
            expected.list_debug(",\n")
        );
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
        _ = rollforward_from(&mut tree, tip, &alice, 5);

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

        let bob = Peer::new("bob");
        let first = &headers[0];
        tree.initialize_peer(&bob, &first.hash()).unwrap();

        // Roll forward with 2 new headers from bob
        let _ = rollforward_from(&mut tree, first, &bob, 2);

        // Now roll forward alice's tip twice
        // Now bob's chain is unreachable and is dropped from the tree
        let _ = rollforward_from(&mut tree, tip, &alice, 2);
        assert_eq!(tree.size(), 10);
    }

    #[test]
    fn dangling_peers_are_removed_when_the_best_chain_grows_too_large() {
        // create a chain at max length for alice
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = create_headers_tree_with_store(store.clone(), 10);
        let headers = tree.best_chain_fragment();
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        tree.initialize_peer(&alice, &tip.hash()).unwrap();

        // start a chain for bob at the root
        let bob = Peer::new("bob");
        tree.initialize_peer(&bob, &headers[0].hash()).unwrap();
        _ = rollforward_from(&mut tree, &headers[0], &bob, 2);

        // Add some headers to the best chain. This should cause
        // the original root node disappear from the arena
        // and bob to stop being tracked
        _ = rollforward_from(&mut tree, tip, &alice, 4);

        // As a consequence it is not possible to add a new header for bob, it has to be
        // registered again
        let next_header = store_header_with_parent(store, tip);
        assert!(tree.select_roll_forward(&bob, &next_header).is_err());
    }

    #[test]
    fn select_rollback_on_the_best_chain_is_no_change() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let middle = &headers[3];
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(tree.best_chain_tip(), headers[4]);
        assert_eq!(result, RollbackChainSelection::NoChange);
    }

    #[test]
    // TODO: this is a case of a peer "stuttering" which can be considered adversarial?
    fn rollback_then_roll_forward_with_same_header_on_single_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // rollback
        let rollback_to = &headers[3];
        tree.select_rollback(&alice, &rollback_to.hash()).unwrap();

        // roll forward again
        let result = tree.select_roll_forward(&alice, &headers[4]).unwrap();
        assert_eq!(result, ForwardChainSelection::NoChange);
        assert_eq!(tree.best_chain_tip(), headers[4])
    }

    #[test]
    fn rollback_then_roll_forward_on_the_best_chain() {
        let alice = Peer::new("alice");
        let store = Arc::new(InMemConsensusStore::new());
        let mut tree = initialize_with_store_and_peer(store.clone(), 5, &alice);

        let headers = tree.best_chain_fragment();
        let middle = &headers[2];

        // Rollback first
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);

        // Then roll forward
        let new_tip = store_header_with_parent(store, middle);
        let result = tree.select_roll_forward(&alice, &new_tip).unwrap();
        assert_eq!(result, ForwardChainSelection::NoChange);
    }

    #[test]
    fn rollback_beyond_limit() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let added_headers = rollforward_from(&mut tree, &headers[4], &alice, 15);

        // try to rollback alice in the past
        let result = tree.select_rollback(&alice, &headers[3].hash()).unwrap();

        let new_tree_anchor = added_headers[5].hash(); // after adding 15 headers, the new anchor is the 6th added header
        assert_eq!(
            result,
            RollbackBeyondLimit {
                peer: alice,
                rollback_point: headers[3].hash(),
                max_point: new_tree_anchor,
            }
        );
    }

    #[test]
    fn rollback_to_the_root() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        let rollback_point = &headers[0];
        let result = tree
            .select_rollback(&alice, &rollback_point.hash())
            .unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);
        assert_eq!(tree.best_chain_tip(), headers[4]);
    }

    #[test]
    fn rollback_is_no_change_even_if_the_rolled_back_chain_stays_the_best() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // alice has the best chain with 5 headers
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // bob branches off on headers[1] and has now the best chain with 6 headers
        let header1 = &headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, header1, &bob, 4); // 4 added headers

        // Now we have
        // 0 - 1 - 2 - 3 - 4 (alice)
        //     + - 5 - 6 - 7 - 6 - 9 (*bob, me)

        // sanity check: bob chain is the longest
        assert_eq!(tree.best_chain_tip(), *added_headers.last().unwrap());

        // Now bob is rolled back 2 headers. The best chain stays at 9
        // but bob is rolled backed to 7
        // 0 - 1 - 2 - 3 - 4 (alice)
        //     + - 5 - 6 - 7 - 6 - 9 (*me)
        //               (bob)
        let result = tree
            .select_rollback(&bob, &added_headers[1].hash())
            .unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);
    }

    #[test]
    fn rollback_is_no_change_until_we_have_a_better_chain_with_2_peers() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        // alice has the best chain with 5 headers
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        // bob branches off on headers[1] and has now the best chain with 6 headers
        let header1 = &headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, header1, &bob, 4); // 4 added headers

        // Now we have
        // 0 - 1 - 2 - 3 - 4 (alice)
        //     + - 5 - 6 - 7 - 8 (*bob, me)

        // sanity check: bob chain is the longest
        assert_eq!(tree.best_chain_tip(), *added_headers.last().unwrap());

        // alice adds one more header. Now bob's chain is the best
        // and both bob and alice are in the best chains list
        //
        // 0 - 1 - 2 - 3 - 4 - 10 (alice)
        //     + - 5 - 6 - 7 - 8 (*bob, me)
        let _ = rollforward_from(&mut tree, &headers[4], &alice, 1);

        // Now bob is rolled back 2 headers.
        // We internally switch to alice's chain as the best
        // 0 - 1 - 2 - 3 - 4 - 10 (*alice)
        //     + - 5 - 6 - 7 - 8 (*me)
        //           (bob)
        let result = tree
            .select_rollback(&bob, &added_headers[1].hash())
            .unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);
    }

    #[test]
    fn rollback_is_no_change_until_we_have_a_better_chain_with_3_peers() {
        // In the case we end-up in this situation:
        //  alice has the best chain and charlie is about to roll to 3
        //  0 - 1 - 2 - 3  used to be bob's best chain, it's now `me`'s best chain
        //
        // 0 (bob)
        // + - 1
        //     + - 2  (charlie)
        //     |   + - 3 (*me)
        //     + - 4
        //         + - 5

        let actions = [
            r#"{"RollForward":{"peer":"1","header":{"hash":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46","block":1,"slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85","block":1,"slot":2,"parent":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf","block":1,"slot":3,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04","block":1,"slot":4,"parent":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"0.d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0","block":1,"slot":4,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"e14eb3b5eeaa2dc35c584d2644757217f5f6e82f17a9eb9be0137044bb2302c5","block":1,"slot":5,"parent":"fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"0.fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46","block":1,"slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85","block":1,"slot":2,"parent":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf","block":1,"slot":3,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04","block":1,"slot":4,"parent":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"0.e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}"#,
            r#"{"RollForward":{"peer":"3","header":{"hash":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46","block":1,"slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":"3","header":{"hash":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85","block":1,"slot":2,"parent":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}}"#,
            r#"{"RollForward":{"peer":"3","header":{"hash":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf","block":1,"slot":3,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":"3","header":{"hash":"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04","block":1,"slot":4,"parent":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf"}}}"#,
        ];

        let results = check_execution(100, &actions, false);
        assert_matches!(
            results.last(),
            Some(Forward(ForwardChainSelection::NoChange))
        );
    }

    #[test]
    fn test_rollforward_with_trim_chain() {
        // This test covers the scenario where two peers are rolling forward and
        // a fork triggers the trimming of the headers tree.
        let actions = [
            r#"{"RollForward":{"peer":"1","header":{"hash":"8acbec9278c423b6a00502d9519b8e445e2b709afdd7a0093044274ba631f83a","block":1,"slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"8acbec9278c423b6a00502d9519b8e445e2b709afdd7a0093044274ba631f83a","block":1,"slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574","block":2,"slot":2,"parent":"8acbec9278c423b6a00502d9519b8e445e2b709afdd7a0093044274ba631f83a"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574","block":2,"slot":2,"parent":"8acbec9278c423b6a00502d9519b8e445e2b709afdd7a0093044274ba631f83a"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db","block":3,"slot":3,"parent":"ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7","block":3,"slot":3,"parent":"ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"377428929c248c39e31989aa5c0931040b2b4a3568cfe8137ddec0a7fc628fa4","block":4,"slot":4,"parent":"992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae","block":4,"slot":4,"parent":"e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"4.992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"49c94d2ca3c6dc00cf111a66db84119373173f252f181117b846ff18fb8e6030","block":5,"slot":5,"parent":"222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566","block":4,"slot":4,"parent":"992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"5.222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"74d30dbdee704742033d5761f86c6a595bfa33db9724dcfd9f649b9784890ed6","block":5,"slot":5,"parent":"7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"586181a92e0f8ec2ec8a37cec708d62e8c97ef57650792e693e4da5bbe0c61fc","block":5,"slot":5,"parent":"222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"5.7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"5.222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"f3fc9434e61c348fa7794951ceaced314b0a1b2e25470e1cbf8e968710752177","block":5,"slot":5,"parent":"7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"4.e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"5.7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"3502f9c76d97c36b5f93e0fb272357be8c5e560832cb1afd5f1d77c97e2b9c66","block":4,"slot":4,"parent":"e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"4.992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"1f0a669cfcde0b6724bbc87c5b1c53cd4da65dbcbe4c0c470f6d9defed7d060f","block":5,"slot":5,"parent":"3502f9c76d97c36b5f93e0fb272357be8c5e560832cb1afd5f1d77c97e2b9c66"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"3.ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574"}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"5.3502f9c76d97c36b5f93e0fb272357be8c5e560832cb1afd5f1d77c97e2b9c66"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7","block":3,"slot":3,"parent":"ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"4.e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae","block":4,"slot":4,"parent":"e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"3.ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"49c94d2ca3c6dc00cf111a66db84119373173f252f181117b846ff18fb8e6030","block":5,"slot":5,"parent":"222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db","block":3,"slot":3,"parent":"ebabaacc3dc4b8a6a3c43c6ca5010f1712ad6b7b12cbf8363f3e218eae62b574"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"5.222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566","block":4,"slot":4,"parent":"992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"586181a92e0f8ec2ec8a37cec708d62e8c97ef57650792e693e4da5bbe0c61fc","block":5,"slot":5,"parent":"222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"74d30dbdee704742033d5761f86c6a595bfa33db9724dcfd9f649b9784890ed6","block":5,"slot":5,"parent":"7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"5.222518db44df8eb9d3a5b9d3f9c20dd4e516708235f52e20489faeefbe9596ae"}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"5.7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}"#,
            r#"{"RollBack":{"peer":"1","rollback_point":"4.e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"f3fc9434e61c348fa7794951ceaced314b0a1b2e25470e1cbf8e968710752177","block":5,"slot":5,"parent":"7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"3502f9c76d97c36b5f93e0fb272357be8c5e560832cb1afd5f1d77c97e2b9c66","block":4,"slot":4,"parent":"e12d4821181f94971c59c2028177031a3356a9c00034f0e0738c9032369362b7"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"5.7d3e516e65d18827535aa6f080a7d1820e4b854a16aa62ab8d92b6f17925f566"}}"#,
            r#"{"RollForward":{"peer":"1","header":{"hash":"1f0a669cfcde0b6724bbc87c5b1c53cd4da65dbcbe4c0c470f6d9defed7d060f","block":5,"slot":5,"parent":"3502f9c76d97c36b5f93e0fb272357be8c5e560832cb1afd5f1d77c97e2b9c66"}}}"#,
            r#"{"RollBack":{"peer":"2","rollback_point":"4.992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}"#,
            r#"{"RollForward":{"peer":"2","header":{"hash":"377428929c248c39e31989aa5c0931040b2b4a3568cfe8137ddec0a7fc628fa4","block":4,"slot":4,"parent":"992e19bfc0fc0a124d4c7878d9229c2bcc3511c56f6fc1052e8d0fde29af51db"}}}"#,
        ];

        check_execution(3, &actions, false);
    }

    #[test]
    #[should_panic(
        expected = "Cannot create a headers tree with maximum chain length lower than 2"
    )]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<BlockHeader>::new_in_memory(1);
    }

    const DEPTH: usize = 10;
    const MAX_LENGTH: usize = 5;
    const TEST_CASES_NB: u32 = 1000;
    const PEERS_NB: usize = 2;

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(TEST_CASES_NB).end())]
        #[test]
        fn run_chain_selection(generated_actions in any_select_chains(DEPTH, PEERS_NB)) {
            let results = execute_actions(MAX_LENGTH, &generated_actions.actions(), false).unwrap();
            let actual = make_best_chain_from_results(&results).unwrap();
            // check the actual best chain is one of the expected best chains
            // as given by the longest chains in the generated tree of headers.
            let expected = generated_actions.generated_tree().best_chains();

            proptest::prop_assert!(expected.contains(&actual),
                "\nthe actual chain\n {}\n\nis not contained in the best chains\n\n{}\n\n",
                               actual.list_to_string(",\n "), expected.lists_to_string(",\n ", "\n\n"));
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
        tree: &mut HeadersTree<BlockHeader>,
        header: &BlockHeader,
        peer: &Peer,
        times: u32,
    ) -> Vec<BlockHeader> {
        let mut result = vec![];
        let mut parent = header.clone();
        for _ in 0..times {
            let next_header = store_header_with_parent(tree.chain_store.clone(), &parent);
            tree.select_roll_forward(peer, &next_header).unwrap();
            result.push(next_header.clone());
            parent = next_header;
        }
        result
    }

    /// Execute a list of actions encoded as JSON strings
    /// and verify, at every step, that the best chain resulting from
    /// the execution is one of the expected best chains.
    ///
    /// The print bool enables printing additional debug information during execution.
    fn check_execution(max_length: usize, actions: &[&str], print: bool) -> Vec<SelectionResult> {
        match check_actions_execution(max_length, actions_from_json(actions), print) {
            Ok(results) => results,
            Err(e) => panic!("{e}"),
        }
    }

    /// Execute a list of actions encoded as JSON strings
    /// and verify, at every step, that the best chain resulting from
    /// the execution is one of the expected best chains.
    ///
    /// The print bool enables printing additional debug information during execution.
    fn check_actions_execution(
        max_length: usize,
        actions: Vec<Action>,
        print: bool,
    ) -> Result<Vec<SelectionResult>, String> {
        let results = execute_actions(max_length, &actions, print).unwrap();
        let actual_chains = make_best_chains_from_results(&results)?;
        let expected_chains = make_best_chains_from_actions(&actions);
        for (i, (actual, expected)) in actual_chains.iter().zip(expected_chains).enumerate() {
            if !expected.contains(actual) {
                return Err(format!(
                    "\nFor action {}, the actual chain\n {}\n\nis not contained in the best chains\n\n{}\n\n",
                    i + 1,
                    actual.list_to_string(",\n "),
                    expected.lists_to_string(",\n ", "\n\n")
                ));
            }
        }
        Ok(results)
    }
}
