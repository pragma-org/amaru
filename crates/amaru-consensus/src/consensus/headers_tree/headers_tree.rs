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
use crate::consensus::headers_tree::headers_tree::Tracker::{Me, SomePeer};
use crate::consensus::headers_tree::tree::Tree;
use crate::consensus::stages::select_chain::RollbackChainSelection::RollbackBeyondLimit;
use crate::consensus::stages::select_chain::{Fork, ForwardChainSelection, RollbackChainSelection};
use amaru_kernel::string_utils::ListToString;
use amaru_kernel::{HEADER_HASH_SIZE, ORIGIN_HASH, Point, peer::Peer};
use amaru_ouroboros_traits::IsHeader;
use amaru_stores::chain_store::ChainStore;
#[cfg(any(test, feature = "test-utils"))]
use amaru_stores::in_memory::consensus::InMemConsensusStore;
use itertools::Itertools;
use pallas_crypto::hash::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display, Formatter};
use std::iter::successors;
use std::sync::Arc;
use tracing::instrument;

/// Type alias for a header hash to improve readability
type HeaderHash = Hash<HEADER_HASH_SIZE>;

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
    /// Maximum size allowed for a given chain
    max_length: usize,
    /// List of headers by their hash
    // headers: BTreeMap<HeaderHash, Tip<H>>,
    /// View of the parent->child relationship for headers.
    parent_child_relationship: BTreeMap<HeaderHash, Vec<HeaderHash>>,
    /// The root of the tree
    root: HeaderHash,
    /// This map maintains the hashes of the headers of each peer chain, starting from the root.
    peers: BTreeMap<Tracker, Vec<HeaderHash>>,
    /// One chain tip designates as THE best chain among several chains of the same length.
    best_chain: HeaderHash,
    chain_store: Arc<dyn ChainStore<H>>,
}

impl<H: PartialEq> PartialEq for HeadersTree<H> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<H: Eq> Eq for HeadersTree<H> {}

impl<H: Serialize> Serialize for HeadersTree<H> {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        todo!("serialize for HeadersTree")
    }
}

impl<'de, H: Deserialize<'de>> Deserialize<'de> for HeadersTree<H> {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        todo!("deserialize for HeadersTree")
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

impl<H: IsHeader + Clone + Debug + PartialEq + Eq> Debug for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.format(f, |tip| format!("{tip:?}"))
    }
}

impl<H: IsHeader + Debug + Clone + Display + PartialEq + Eq> Display for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.format(f, |tip| format!("{tip}"))
    }
}

impl<H: IsHeader + Debug + Clone + PartialEq + Eq> HeadersTree<H> {
    /// Common function to format the headers tree either for Debug or Display
    fn format(
        &self,
        f: &mut Formatter<'_>,
        header_to_string: fn(&H) -> String,
    ) -> std::fmt::Result {
        f.write_str("HeadersTree {\n")?;
        if let Some(tree) = self.to_tree() {
            writeln!(
                f,
                "  headers:\n    {}",
                &tree.pretty_print_with(header_to_string)
            )?;
        }
        writeln!(f, "  root: {}", &self.root)?;
        writeln!(
            f,
            "  peers: {}",
            &self
                .peers
                .iter()
                .map(|(p, hs)| format!("{} -> [{}]", p, hs.list_to_string(", ")))
                .join(", ")
        )?;
        writeln!(f, "  best_chain: {}", &self.best_chain())?;
        writeln!(
            f,
            "  best_chains: [{}]",
            &self.best_chains().list_to_string(", ")
        )?;
        writeln!(f, "  best_length: {}", &self.best_length())?;
        writeln!(f, "  max_length: {}", &self.max_length)?;
        f.write_str("}\n")
    }

    /// Return the tree representation of the headers tree.
    fn to_tree(&self) -> Option<Tree<H>> {
        let mut as_map = BTreeMap::new();
        for header in self.chain_store.load_headers() {
            let _ = as_map.insert(header.hash(), header);
        }
        Tree::from(&as_map)
    }
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq + Send + Sync + 'static> HeadersTree<H> {
    /// Initialize a HeadersTree as a tree rooted in the Genesis header
    pub fn new(
        chain_store: Arc<dyn ChainStore<H>>,
        max_length: usize,
        root: &Option<H>,
    ) -> HeadersTree<H> {
        HeadersTree::create(chain_store, max_length, root.clone())
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_in_memory(max_length: usize, root: &Option<H>) -> HeadersTree<H> {
        HeadersTree::new(Arc::new(InMemConsensusStore::new()), max_length, root)
    }

    /// Create a new HeadersTree with a specific tip as the root
    fn create(
        chain_store: Arc<dyn ChainStore<H>>,
        max_length: usize,
        header: Option<H>,
    ) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );

        let mut peers = BTreeMap::new();
        if let Some(header) = header {
            let hash = header.hash();
            let _ = chain_store.store_header(&hash, &header);
            peers.insert(Me, vec![hash]);
        }

        HeadersTree {
            max_length,
            parent_child_relationship: BTreeMap::new(),
            root: ORIGIN_HASH,
            peers,
            best_chain: ORIGIN_HASH,
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
            self.peers.insert(SomePeer(peer.clone()), peer_chain);
            Ok(())
        } else {
            Err(UnknownPoint(*hash))
        }
    }

    /// Add a new header coming from an upstream peer to the tree.
    /// The result will be an event describing:
    ///   - No change (if the header is already known for example).
    ///   - A NewTip .
    ///   - A Fork if the chain that ends with this new header becomes the longest one.
    ///
    /// There will be errors if:
    ///   - The header's parent is unknown
    ///
    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        tip: H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        // The header must be a child of the peer's tip
        if !self.is_tip_child(peer, &tip) {
            let e = ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer.clone(),
                forwarded: tip.point(),
                actual: tip.parent(),
                expected: self.get_point(peer)?,
            }));
            return Err(e);
        };

        let result = if self.is_empty_tree() {
            self.insert_header(&tip)?;
            self.update_peer(peer, &tip);
            self.best_chain = tip.hash();
            self.update_me();
            Ok(ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip: tip.clone(),
            })
        } else {
            self.insert_header(&tip)?;
            // If the tip extends one of the best chains
            if self
                .best_chains()
                .iter()
                .any(|h| Some(*h) == tip.parent().as_ref())
            {
                self.update_peer(peer, &tip);
                // If the tip is extending _the_ best chain
                let result = if tip.parent().as_ref() == Some(&self.best_chain) {
                    Ok(ForwardChainSelection::NewTip {
                        peer: peer.clone(),
                        tip: tip.clone(),
                    })
                } else {
                    let fork = self.make_fork(peer, &self.best_chain, &tip.hash());
                    Ok(ForwardChainSelection::SwitchToFork(fork))
                };
                self.best_chain = tip.hash();
                self.update_me();
                result
            } else {
                self.update_peer(peer, &tip);
                Ok(ForwardChainSelection::NoChange)
            }
        };
        self.prune_headers()?;
        result
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
        if !self.chain_store.has_header(rollback_hash) {
            return Ok(RollbackBeyondLimit {
                peer: peer.clone(),
                rollback_point: *rollback_hash,
                max_point: self.root_hash(),
            });
        }

        // The peer tip must be known
        if let Some(peer_tip) = self.get_peer_tip(peer).cloned() {
            // This is a no-op but possibly we need to flag the peer as adversarial
            if &peer_tip == rollback_hash {
                return Ok(RollbackChainSelection::NoChange);
            }

            // Remove invalid headers for that peer
            self.trim_chain(peer, rollback_hash)?;

            if self.best_chain == peer_tip {
                // recompute the best chains
                let best_chains = self.best_chains();

                // Otherwise we switch to a better chain -> Switch to fork
                self.best_chain = **best_chains.first().unwrap_or(&&self.best_chain);
                if let Some(best_peer) = self.best_peer() {
                    let fork = self.make_fork(&best_peer, rollback_hash, &self.best_chain);
                    self.update_me();
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

    /// Update "Me" so that it points to the best chain, as given by the the best chain tip
    fn update_me(&mut self) {
        self.peers.insert(Me, self.best_chain_fragment_hashes());
    }

    /// Insert a header into the tree and the map of headers.
    ///
    fn insert_header(&mut self, header: &H) -> Result<(), ConsensusError> {
        // add the new header to the map
        self.chain_store
            .store_header(&header.hash(), header)
            .map_err(|e| ConsensusError::StoreHeaderFailed(header.hash(), e))?;
        // add the new header to the tree
        if let Some(parent_hash) = header.parent() {
            self.parent_child_relationship
                .entry(parent_hash)
                .or_default()
                .push(header.hash());
        } else if self.is_empty_tree() {
            self.chain_store
                .remove_header(&ORIGIN_HASH)
                .map_err(|e| ConsensusError::RemoveHeaderFailed(header.hash(), e))?;
            self.peers.remove(&Me);
            self.best_chain = header.hash();
            self.root = header.hash();
        } else {
            // We just need to check that the header is the root header
            if header.hash() != self.root {
                return Err(UnknownPoint(header.hash()));
            };
        }
        Ok(())
    }
}

impl<H: IsHeader + Clone + PartialEq + Eq> HeadersTree<H> {
    /// Return the length of the best chain currently known.
    pub fn best_length(&self) -> usize {
        self.peers
            .values()
            .map(|chain| chain.len())
            .max()
            .unwrap_or_default()
    }

    /// Return the tracker of the best chain, or "Me" if no peer is tracking it.
    fn best_tracker(&self) -> &Tracker {
        self.peers
            .iter()
            .find(|(p, chain)| p != &&Me && chain.last() == Some(&self.best_chain))
            .map(|pc| pc.0)
            .unwrap_or(&Me)
    }

    /// Return the peer tracking the best chain, or None if "Me" is tracking it.
    fn best_peer(&self) -> Option<Peer> {
        self.best_tracker().to_peer()
    }

    /// Return the list of the best chains currently known.
    fn best_chains(&self) -> Vec<&HeaderHash> {
        let mut best_length = 0;
        let mut best_chains = vec![];
        for chain in self.peers.values() {
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
        self.root == ORIGIN_HASH
    }

    /// Remove all the header hashes for a peer chain after (and excluding) the given hash.
    /// Return an error if the hash is not found in the peer chain.
    fn trim_chain(&mut self, peer: &Peer, hash: &HeaderHash) -> Result<(), ConsensusError> {
        if let Some(chain) = self.peers.get_mut(&SomePeer(peer.clone())) {
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
            }
        }
        fork_fragment.reverse();

        Fork {
            peer: best_peer.clone(),
            rollback_point: self.unsafe_get_header(&intersection_hash).point(),
            fork: fork_fragment,
        }
    }

    /// Update the peer chain with the new tip
    fn update_peer(&mut self, peer: &Peer, tip: &H) {
        let tracker = SomePeer(peer.clone());
        if let Some(chain) = self.peers.get_mut(&tracker) {
            chain.push(tip.hash());
        } else {
            self.peers.insert(tracker, vec![tip.hash()]);
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

    /// Return the ancestors of the header, including the header itself.
    fn ancestors(&self, start: &H) -> impl Iterator<Item = H> + use<'_, H> {
        successors(Some(start.clone()), move |h| {
            h.parent().and_then(|p| self.get_header(&p))
        })
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors_hashes(&self, hash: &HeaderHash) -> Box<dyn Iterator<Item = HeaderHash> + '_> {
        if let Some(header) = self.get_header(hash) {
            Box::new(self.ancestors(&header).map(|h| h.hash()))
        } else {
            Box::new(std::iter::empty())
        }
    }

    /// Return root of the tree
    fn root_hash(&self) -> HeaderHash {
        self.root
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
        self.peers
            .get(&SomePeer(peer.clone()))
            .and_then(|hs| hs.last())
    }

    /// Return the best currently known tip
    fn best_chain(&self) -> &HeaderHash {
        &self.best_chain
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
        self.root_hash()
    }

    /// When the best chain exceeds the maximum length, move its root up one level and
    /// remove all the subtrees that start from the old root.
    #[instrument(level = "trace", skip_all)]
    fn prune_headers(&mut self) -> Result<(), ConsensusError> {
        if self.best_length() <= self.max_length || self.chain_store.count_headers() <= 2 {
            return Ok(());
        }

        // Get the first two headers of the best chain fragment
        let (first, second) = {
            let mut best_chain_fragment = self.best_chain_fragment_hashes_iterator();
            (best_chain_fragment.next(), best_chain_fragment.next())
        };

        // If we have at least two headers, we can move the root up one level
        if let (Some(first), Some(second)) = (first, second) {
            // Now second becomes the new root and we need to delete all the subtrees that are not starting from it
            let mut other_roots: Vec<_> =
                if let Some(children) = self.parent_child_relationship.get(&first) {
                    children.iter().filter(|t| **t != second).cloned().collect()
                } else {
                    vec![]
                };

            // The original root needs to be deleted as well but not its children.
            other_roots.push(first);
            self.parent_child_relationship.insert(first, vec![]);

            // Follow the parent-child relationship for the other roots and
            //  - Delete their headers from the headers map
            //  - Delete the parent-child edges from the relationship map
            //
            // Update the set of removed hashes.
            let mut removed_hashes = BTreeSet::new();
            for other_root in other_roots {
                self.delete_recursively(&other_root, &mut removed_hashes)?;
            }

            // Use the list of removed hashes to update the peer chains.
            // Remove the peers that are pointing to unreachable headers
            self.peers.iter_mut().for_each(|(_p, hs)| {
                hs.retain(|h| !removed_hashes.contains(h));
            });
            self.peers.retain(|_p, hs| !hs.is_empty());

            // Set the new root
            self.root = second;
        };
        Ok(())
    }

    /// Remove children headers hashes, starting from a given root in the parent_child_relationship map.
    /// Collect the removed hashes in the given set.
    fn delete_recursively(
        &mut self,
        hash: &HeaderHash,
        removed_hashes: &mut BTreeSet<HeaderHash>,
    ) -> Result<(), ConsensusError> {
        if let Some(children) = self.parent_child_relationship.get(hash).cloned() {
            for c in children.iter() {
                self.delete_recursively(c, removed_hashes)?;
            }
        }
        self.chain_store
            .remove_header(hash)
            .map_err(|e| ConsensusError::StoreHeaderFailed(*hash, e))?;
        self.parent_child_relationship.remove(hash);
        removed_hashes.insert(*hash);
        Ok(())
    }

    /// Return the best chain fragment currently known as a list of hashes.
    /// The list starts from the root.
    fn best_chain_fragment_hashes(&self) -> Vec<HeaderHash> {
        self.peers
            .values()
            .find(|chain| chain.last() == Some(&self.best_chain))
            .cloned()
            .unwrap_or_default()
    }

    /// Return the best chain fragment currently known as a list of hashes.
    /// The list starts from the root.
    fn best_chain_fragment_hashes_iterator(&self) -> impl Iterator<Item = HeaderHash> {
        self.peers
            .values()
            .find(|chain| chain.last() == Some(&self.best_chain))
            .map(|chain| chain.iter().cloned())
            .unwrap_or_default()
    }
}

#[cfg(any(test, doc, feature = "test-utils"))]
/// Those functions are only used by tests
impl<H: IsHeader + Clone + Debug + PartialEq + Eq + Send + Sync + 'static> HeadersTree<H> {
    /// Return true if the peer is known
    pub fn has_peer(&self, peer: &Peer) -> bool {
        self.peers.contains_key(&SomePeer(peer.clone()))
    }

    /// Return the tip of the best chain that currently known as a header
    pub fn best_chain_tip(&self) -> H {
        self.unsafe_get_header(self.best_chain())
    }

    /// Return the best chain fragment currently known as a list of headers.
    /// The list starts from the root.
    pub fn best_chain_fragment(&self) -> Vec<H> {
        let mut fragment: Vec<_> = self
            .ancestors(&self.unsafe_get_header(&self.best_chain))
            .collect();
        fragment.reverse();
        fragment
    }

    /// Return the headers tree size in terms of how many headers are being tracked.
    /// This is used to check the garbage collection aspect of this data structure.
    #[cfg(test)]
    fn size(&self) -> usize {
        self.chain_store.count_headers()
    }

    /// Insert headers into the arena and return the last created node id
    /// This function is used to initialize the tree from persistent storage.
    pub(crate) fn insert_headers(&mut self, headers: &[H]) -> Result<(), ConsensusError> {
        for header in headers {
            self.insert_header(header)?;
        }
        if let Some(last) = headers.last() {
            self.best_chain = last.hash();
        }

        self.peers
            .insert(Me, headers.iter().map(|h| h.hash()).collect());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::headers_tree::data_generation::SelectionResult::Forward;
    use crate::consensus::headers_tree::data_generation::*;
    use crate::consensus::stages::select_chain::Fork;
    use crate::consensus::stages::select_chain::ForwardChainSelection::SwitchToFork;
    use amaru_kernel::ORIGIN_HASH;
    use amaru_kernel::string_utils::{ListDebug, ListsToString};
    use proptest::{prop_assert, proptest};
    use std::assert_matches::assert_matches;

    #[test]
    fn initialize_peer_on_empty_tree() {
        let mut tree: HeadersTree<TestHeader> = HeadersTree::new_in_memory(10, &None);
        let peer = Peer::new("alice");
        let mut header = generate_header();
        header.parent = Some(ORIGIN_HASH);
        tree.initialize_peer(&peer, &Point::Origin.hash()).unwrap();
        tree.select_roll_forward(&peer, header).unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            header,
            "there must be a best chain available after the first roll forward"
        );
    }

    #[test]
    fn single_chain_is_best_chain() {
        let headers = generate_headers_chain(5);
        let last = headers[4];
        let mut tree = HeadersTree::new_in_memory(10, &None);
        tree.insert_headers(&headers).unwrap();

        assert_eq!(tree.size(), 5);
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
        assert_eq!(tree.best_chain_tip(), new_tip);

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

        let result = tree.select_roll_forward(&bob, bob_new_tip).unwrap();
        assert_eq!(result, SwitchToFork(fork));
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
        assert_eq!(tree.best_chain_tip(), new_tip);

        // The expected chain ends with the new tip but starts with the second header
        // in order to stay below max_length
        headers.push(new_tip);
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
        let first = headers[0];
        tree.initialize_peer(&bob, &first.hash()).unwrap();

        // Roll forward with 2 new headers from bob
        let _ = rollforward_from(&mut tree, &first, &bob, 2);

        // Now roll forward alice's tip twice
        // Now bob's chain is unreachable and is dropped from the tree
        let _ = rollforward_from(&mut tree, tip, &alice, 2);
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
        let next_header = make_header_with_parent(tip);
        assert!(tree.select_roll_forward(&bob, next_header).is_err());
    }

    #[test]
    fn select_rollback_on_the_best_chain_is_no_change() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let middle = headers[3];
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);
        assert_eq!(tree.best_chain_tip(), headers[4]);
    }

    #[test]
    // TODO: this is a case of a peer "stuttering" which can be considered adversarial?
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
        assert_eq!(tree.best_chain_tip(), headers[4])
    }

    #[test]
    fn rollback_then_roll_forward_on_the_best_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let middle = headers[2];

        // Rollback first
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackChainSelection::NoChange);

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
        tree.initialize_peer(&bob, &headers[4].hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &headers[4], &alice, 15);

        let result = tree.select_rollback(&bob, &headers[3].hash()).unwrap();

        let new_tree_root = added_headers[5].hash(); // after adding 15 headers, the new root is the 6th added header
        assert_eq!(
            result,
            RollbackBeyondLimit {
                peer: bob,
                rollback_point: headers[3].hash(),
                max_point: new_tree_root,
            }
        );
    }

    #[test]
    fn rollback_to_the_root() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();

        let rollback_point = headers[0];
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
        let header1 = headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &header1, &bob, 4); // 4 added headers

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
        let header1 = headers[1];
        tree.initialize_peer(&bob, &header1.hash()).unwrap();
        let added_headers = rollforward_from(&mut tree, &header1, &bob, 4); // 4 added headers

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
            r#"{"RollForward":{"peer":{"name":"1"},"header":{"hash":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46","slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":{"name":"1"},"header":{"hash":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85","slot":2,"parent":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}}"#,
            r#"{"RollForward":{"peer":{"name":"1"},"header":{"hash":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf","slot":3,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":{"name":"1"},"header":{"hash":"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04","slot":4,"parent":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf"}}}"#,
            r#"{"RollBack":{"peer":{"name":"1"},"rollback_point":"0.d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}"#,
            r#"{"RollForward":{"peer":{"name":"1"},"header":{"hash":"fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0","slot":4,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":{"name":"1"},"header":{"hash":"e14eb3b5eeaa2dc35c584d2644757217f5f6e82f17a9eb9be0137044bb2302c5","slot":5,"parent":"fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0"}}}"#,
            r#"{"RollBack":{"peer":{"name":"1"},"rollback_point":"0.fe52c3448ad441b3ea05321637e3a25d2c1efe8dfaa103d71a0ca76726fd38f0"}}"#,
            r#"{"RollForward":{"peer":{"name":"2"},"header":{"hash":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46","slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":{"name":"2"},"header":{"hash":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85","slot":2,"parent":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}}"#,
            r#"{"RollForward":{"peer":{"name":"2"},"header":{"hash":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf","slot":3,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":{"name":"2"},"header":{"hash":"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04","slot":4,"parent":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf"}}}"#,
            r#"{"RollBack":{"peer":{"name":"2"},"rollback_point":"0.e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}"#,
            r#"{"RollForward":{"peer":{"name":"3"},"header":{"hash":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46","slot":1,"parent":null}}}"#,
            r#"{"RollForward":{"peer":{"name":"3"},"header":{"hash":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85","slot":2,"parent":"e60a1a517c702dccc89677ec23d275510d102c0714418c4668aa1e693a763b46"}}}"#,
            r#"{"RollForward":{"peer":{"name":"3"},"header":{"hash":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf","slot":3,"parent":"d9dee067701868d437ac0e0b582318bafe0ea21346f47d9e50b0a34643762d85"}}}"#,
            r#"{"RollForward":{"peer":{"name":"3"},"header":{"hash":"c5132318d38536501a886ed85652242083a81e922b8f79b9ea2a726315028f04","slot":4,"parent":"85e972660750a9f00e07abde7731c2343ab1b7d9a5edc0e5ff820227fdba3fbf"}}}"#,
        ];

        let results = execute_json_actions(10, &actions, false).unwrap();
        assert_matches!(
            results.last(),
            Some(Forward(ForwardChainSelection::NoChange))
        );
    }

    #[test]
    #[should_panic(
        expected = "Cannot create a headers tree with maximum chain length lower than 2"
    )]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<TestHeader>::new_in_memory(1, &None);
    }

    const DEPTH: usize = 10;
    const MAX_LENGTH: usize = 5;
    const TEST_CASES_NB: u32 = 1000;
    const ROLLBACK_RATIO: Ratio = Ratio(1, 2);

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(TEST_CASES_NB).end())]
        #[test]
        fn run_chain_selection(actions in any_select_chains(DEPTH, ROLLBACK_RATIO)) {
            let results = execute_actions(MAX_LENGTH, &actions, false).unwrap();
            let actual_chains = make_best_chains_from_results(&results);
            let expected_chains = make_best_chains_from_actions(&actions);
            for (i, (actual, expected)) in actual_chains.iter().zip(expected_chains).enumerate() {
                prop_assert!(expected.contains(actual), "\nFor action {}, the actual chain\n{}\n\nis not contained in the best chains\n\n{}\n\n", i+1,
                    actual.list_to_string(",\n "), expected.lists_to_string(",\n ", "\n "));
            }
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
