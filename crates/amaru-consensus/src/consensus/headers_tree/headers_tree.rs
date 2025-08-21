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
use crate::ConsensusError::UnknownPoint;
use crate::{ConsensusError, InvalidHeaderParentData};
use amaru_kernel::{peer::Peer, Point, HEADER_HASH_SIZE, ORIGIN_HASH};
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
    peers: BTreeMap<Peer, Vec<HeaderHash>>,
    /// One chain is always designated as the best chain.
    best_chain: HeaderHash,
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq> Debug for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeadersTree {\n")?;
        f.write_fmt(format_args!("  headers:\n    {:?}\n", &self.to_tree()))?;
        f.write_fmt(format_args!(
            "  peers: {}\n",
            &self
                .peers
                .iter()
                .map(|(p, hs)| format!("{} -> [{}]", p, hs.iter().list_to_string(", ")))
                .join(", ")
        ))?;
        f.write_fmt(format_args!("  best_chain: {}\n", &self.best_chain()))?;
        f.write_fmt(format_args!(
            "  best_chains: [{}]\n",
            &self.best_chains().list_to_string(", ")
        ))?;
        f.write_fmt(format_args!("  best_length: {}\n", &self.best_length()))?;
        f.write_fmt(format_args!("  max_length: {}\n", &self.max_length))?;
        f.write_str("}\n")
    }
}

impl<H: IsHeader + Clone + Debug + Display + PartialEq + Eq> Display for HeadersTree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeadersTree {\n")?;
        f.write_fmt(format_args!("  headers:\n    {}\n", &self.to_tree()))?;
        f.write_fmt(format_args!(
            "  peers: {}\n",
            &self
                .peers
                .iter()
                .map(|(p, hs)| format!("{} -> [{}]", p, hs.list_to_string(", ")))
                .join(", ")
        ))?;
        f.write_fmt(format_args!("  best_chain: {}\n", &self.best_chain()))?;
        f.write_fmt(format_args!(
            "  best_chains: [{}]\n",
            &self.best_chains().list_to_string(", ")
        ))?;
        f.write_fmt(format_args!("  best_length: {}\n", &self.best_length()))?;
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
            best_chain: header.hash(),
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
            let mut peer_chain = self.ancestors_hashes(hash);
            peer_chain.reverse();
            self.peers.insert(peer.clone(), peer_chain);
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
    ///   - The header's parent is unknown
    ///
    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        header: H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
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

        self.select_best_chain_after_forward(peer, &header)
    }

    fn insert_header(&mut self, header: &H) -> Result<(), ConsensusError> {
        // add the new header to the map
        self.headers.insert(header.hash(), Tip::Hdr(header.clone()));
        // add the new header to the tree
        if let Some(parent_hash) = header.parent() {
            assert!(
                self.tree.add(parent_hash, &Tip::Hdr(header.clone())),
                "the header {header:?} must be added to the tree at {parent_hash}"
            );
        } else if self.is_empty_tree() {
            self.headers.remove(&ORIGIN_HASH);
            self.tree = Tree::make_leaf(&Tip::Hdr(header.clone()));
        } else {
            // We just need to check that the header is the root header
            if header.hash() != self.tree.value.hash() {
                return Err(UnknownPoint(header.hash()));
            };
        }
        Ok(())
    }

    fn best_length(&self) -> usize {
        if self.peers.is_empty() {
            self.tree.longest_path().len()
        } else {
            self.peers.iter().map(|pc| pc.1.len()).max().unwrap_or(0)
        }
    }

    fn best_chains(&self) -> Vec<&HeaderHash> {
        if self.peers.is_empty() {
            // FIXME for correctness, gather all the longest paths in the tree
            self.tree
                .longest_path()
                .into_iter()
                // FIXME: is there a better way to get a reference to the header hash?
                .filter_map(|tip| {
                    tip.to_header()
                        .and_then(|header| self.headers.keys().find(|h| *h == &header.hash()))
                })
                .next_back()
                .into_iter()
                .collect()
        } else {
            let best_length = self.best_length();
            self.peers
                .iter()
                .filter(|(_peer, chain)| chain.len() == best_length)
                .filter_map(|(_peer, chain)| chain.last())
                .sorted()
                .collect()
        }
    }

    fn best_peer(&self) -> Option<&Peer> {
        let best_length = self.best_length();
        self.peers
            .iter()
            .filter(|(_peer, chain)| chain.len() == best_length)
            .sorted_by_key(|(_peer, chain)| chain.last())
            .collect::<Vec<_>>()
            .first()
            .map(|(peer, _chain)| *peer)
    }

    /// Return true if the tree is empty, i.e. it only contains the origin header.
    fn is_empty_tree(&self) -> bool {
        self.tree.value.hash() == ORIGIN_HASH
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
        let result = if self.is_empty_tree() {
            self.best_chain = tip.hash();
            Ok(ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip: tip.clone(),
            })
        } else {
            let best_chains = self.best_chains().into_iter().copied().collect::<Vec<_>>();
            let best_chain = self.best_chain;

            // If the tip extends one of the best chains
            let result = if best_chains.iter().any(|h| Some(h) == tip.parent().as_ref()) {
                // If the tip is extending _the_ best chain
                if tip.parent().as_ref() == Some(&best_chain) {
                    self.best_chain = tip.hash();
                    Ok(ForwardChainSelection::NewTip {
                        peer: peer.clone(),
                        tip: tip.clone(),
                    })
                } else {
                    self.best_chain = tip.hash();
                    let previous_best_tip = self.unsafe_get_header(&best_chain).clone();
                    let fork = self.make_fork(peer, &previous_best_tip, tip);
                    Ok(ForwardChainSelection::SwitchToFork(fork))
                }
            } else {
                Ok(ForwardChainSelection::NoChange)
            };
            // Prune old headers if the best chain is now too long
            self.prune_headers();
            result
        };
        self.insert_header(tip)?;
        self.update_peer(peer, tip);
        self.prune_headers();
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
        // The peer must be known
        if let Some(peer_tip) = self.get_peer_tip(peer).cloned() {
            if self.headers.contains_key(rollback_hash) {
                self.select_best_chain_after_rollback(peer, &peer_tip, rollback_hash)
            } else {
                Ok(RollbackBeyondLimit {
                    peer: peer.clone(),
                    rollback_point: *rollback_hash,
                    max_point: self.root_hash(),
                })
            }
        } else {
            Err(ConsensusError::UnknownPeer(peer.clone()))
        }
    }

    /// Determine what is the best chain after a rollback, update the tracking data
    /// and return a rollback chain selection result
    ///
    /// Make sure that arena nodes which are not part of any chain are removed from the arena.
    #[allow(clippy::expect_used)]
    fn select_best_chain_after_rollback(
        &mut self,
        peer: &Peer,
        peer_tip: &HeaderHash,
        rollback_hash: &HeaderHash,
    ) -> Result<RollbackChainSelection<H>, ConsensusError> {
        // This is a no-op but possibly we need to flag the peer as adversarial
        if peer_tip == rollback_hash {
            return Ok(RollbackChainSelection::NoChange);
        }

        // Remove invalid headers for that peer
        if let Some(chain) = self.peers.get_mut(peer) {
            if let Some(rollback_index) = chain.iter().position(|h| h == rollback_hash) {
                // keep everything up to and including the rollback hash
                chain.truncate(rollback_index + 1);
            } else {
                return Err(UnknownPoint(*rollback_hash));
            }
        };

        if &self.best_chain == peer_tip {
            // recompute the best chains
            let best_chains = self.best_chains();

            // If we keep the same best chain -> Rollback
            if best_chains.contains(&rollback_hash) {
                self.best_chain = *rollback_hash;
                Ok(RollbackChainSelection::RollbackTo(*rollback_hash))
            } else {
                // Otherwise we switch to a better chain -> Rollback
                let new_best_chain = *best_chains.first().expect("there must be a best chain");
                let fork = self.make_fork(
                    self.best_peer().unwrap_or(peer),
                    self.unsafe_get_header(rollback_hash),
                    self.unsafe_get_header(new_best_chain),
                );
                self.best_chain = *new_best_chain;
                Ok(RollbackChainSelection::SwitchToFork(fork))
            }
        } else {
            // Otherwise the change did not affect the best chain -> NoChange
            Ok(RollbackChainSelection::NoChange)
        }
    }

    /// Create a fork for a given peer:
    ///  - the old tip is the tip of the previous best chain
    ///  - the new tip is the tip of the new best chain
    fn make_fork(&self, best_peer: &Peer, old_tip: &H, new_tip: &H) -> Fork<H> {
        let intersection_header = self.find_intersection_header(old_tip, new_tip);

        // get all the hashes between the new tip and the forking hash
        let mut fork_fragment: Vec<H> = vec![];
        let mut current = new_tip;
        while current.hash() != intersection_header.hash() {
            if let Some(parent) = current.parent() {
                fork_fragment.push(current.clone());
                current = self.unsafe_get_header(&parent);
            }
        }
        fork_fragment.reverse();

        // return the fork
        Fork {
            peer: best_peer.clone(),
            rollback_point: intersection_header.point(),
            fork: fork_fragment,
        }
    }

    fn update_peer(&mut self, peer: &Peer, tip: &H) {
        if let Some(chain) = self.peers.get_mut(peer) {
            chain.push(tip.hash());
        } else {
            self.peers.insert(peer.clone(), vec![tip.hash()]);
        }
    }

    #[allow(clippy::panic)]
    fn unsafe_get_header(&self, hash: &HeaderHash) -> &H {
        self.get_header(hash)
            .unwrap_or_else(|| panic!("A header must exist for hash {}", hash))
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
    fn ancestors<'a>(&'a self, header: &'a H) -> Vec<&'a H> {
        let mut result = vec![header];
        let mut current = header.parent();
        while let Some(parent_hash) = current {
            if let Some(parent_header) = self.get_header(&parent_hash) {
                result.push(parent_header);
                current = parent_header.parent();
            } else {
                current = None;
            }
        }
        result
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors_hashes(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        if let Some(header) = self.get_header(hash) {
            self.ancestors(header).iter().map(|h| h.hash()).collect()
        } else {
            vec![]
        }
    }

    /// Return the depth of the header hash in the tree.
    fn depth(&self, hash: &HeaderHash) -> usize {
        let mut result = 1;
        let mut current = self.unsafe_get_header(hash).parent();
        while let Some(parent_hash) = current {
            if let Some(parent_header) = self.get_header(&parent_hash) {
                result += 1;
                current = parent_header.parent();
            } else {
                current = None;
            }
        }
        result
    }

    /// Return root of the tree
    fn root_hash(&self) -> HeaderHash {
        self.tree.value.hash()
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

    fn get_peer_tip(&self, peer: &Peer) -> Option<&HeaderHash> {
        self.peers.get(peer).and_then(|hs| hs.last())
    }

    /// Return true if the header has already been added to the arena (and not rolled-back)
    fn header_exists(&self, header: &H) -> bool {
        self.headers.contains_key(&header.hash())
    }

    /// Return the hash of the best header of a registered peer
    /// and return an error if the peer is not known.
    fn get_point(&self, peer: &Peer) -> Result<Point, ConsensusError> {
        let tip = self
            .get_peer_tip(peer)
            .ok_or(ConsensusError::UnknownPeer(peer.clone()))?;
        Ok(self.unsafe_get_header(tip).point())
    }

    /// Return the best currently known tip
    fn best_chain(&self) -> &HeaderHash {
        &self.best_chain
    }

    /// Return the tip of the best chain that currently known as a header
    fn best_chain_tip(&self) -> &H {
        self.unsafe_get_header(self.best_chain())
    }

    /// Return the chain root header hash
    pub(crate) fn get_root_hash(&self) -> Option<HeaderHash> {
        if let Some(tip) = self.best_chains().first() {
            if let Some(tip) = self.get_header(tip) {
                self.ancestors_hashes(&tip.hash()).first().cloned()
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Return the header hash that is the least common parent between 2 headers in the tree
    #[allow(clippy::panic)]
    fn find_intersection_header<'a>(&'a self, header1: &'a H, header2: &'a H) -> &'a H {
        let mut ancestors1 = self.ancestors(header1);
        let mut ancestors2 = self.ancestors(header2);
        ancestors1.reverse();
        ancestors2.reverse();
        ancestors1
            .into_iter()
            .zip(ancestors2)
            .take_while(|(n1, n2)| n1.hash() == n2.hash())
            .last()
            .map(|ns| ns.0).unwrap_or_else(move || panic!("by construction a tree must always have the same root for all chains. Found none for {} and {}", header1.hash(), header2.hash()))
    }

    #[allow(clippy::panic)]
    fn prune_headers(&mut self) {
        if self.best_length() <= self.max_length || self.headers.len() <= 2 {
            return;
        }

        let best_chain_fragment = self.best_chain_fragment_hashes();
        let mut removed_hashes = vec![];
        if let &[first, second, ..] = best_chain_fragment.as_slice() {
            self.headers.remove(&first);
            removed_hashes.push(first);
            // now second becomes the new root and we need to delete all the subtrees that are not starting from it
            let other_roots: Vec<&Tree<Tip<H>>> = self
                .tree
                .children
                .iter()
                .filter(|t| t.value.hash() != second)
                .collect();
            for other_root in other_roots {
                for hash in other_root.hashes() {
                    self.headers.remove(&hash);
                    removed_hashes.push(hash);
                }
                removed_hashes.push(other_root.value.hash());
                self.headers.remove(&other_root.value.hash());
            }
            // Remove the peers that are pointing to unreachable headers
            self.peers.iter_mut().for_each(|(_p, hs)| {
                hs.retain(|h| !removed_hashes.contains(h));
            });
            self.peers.retain(|_p, hs| !hs.is_empty());

            // TODO find a way to make this more efficient rather than rebuilding the whole tree
            self.tree = Tree::from(&self.headers);
        };
    }

    /// Return the best chain fragment currently known as a list of hashes.
    /// The list starts from the root.
    fn best_chain_fragment_hashes(&self) -> Vec<HeaderHash> {
        let mut result = self.ancestors_hashes(&self.best_chain_tip().hash());
        result.reverse();
        result
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
        if self.peers.is_empty() {
            self.tree
                .longest_path()
                .into_iter()
                .filter_map(|tip| tip.to_header().cloned())
                .collect()
        } else if let Some(best_peer) = self
            .peers
            .iter()
            .find(|(_peer, chain)| chain.last() == Some(&self.best_chain))
            .map(|(peer, _)| peer)
        {
            self.peers
                .get(best_peer)
                .map(|vs| {
                    vs.iter()
                        .filter_map(|hash| self.get_header(hash).cloned())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        }
    }

    /// Return the headers tree size in terms of how many headers are being tracked.
    /// This is used to check the garbage collection aspect of this data structure.
    fn size(&self) -> usize {
        self.headers.len()
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
        Ok(())
    }
}

pub trait ListToString {
    fn list_to_string(&self, separator: &str) -> String;
}

pub trait ListsToString {
    fn lists_to_string(&self, intra_separator: &str, inter_separator: &str) -> String;
}

pub trait ListDebug {
    fn list_debug(&self, separator: &str) -> String;
}

impl<H: Clone + Display, I: IntoIterator<Item=H> + Clone> ListToString for I {
    fn list_to_string(&self, separator: &str) -> String {
        self.clone()
            .into_iter()
            .map(|h| h.to_string())
            .join(separator)
    }
}

impl<H: Clone + Display, I: IntoIterator<Item=H> + Clone, J: IntoIterator<Item=I> + Clone>
ListsToString for J
{
    fn lists_to_string(&self, intra_separator: &str, inter_separator: &str) -> String {
        self.clone()
            .into_iter()
            .map(|l| format!("[{}]", l.list_to_string(intra_separator)))
            .collect::<Vec<_>>()
            .list_to_string(inter_separator)
    }
}

impl<H: Clone + Debug, I: IntoIterator<Item=H> + Clone> ListDebug for I {
    fn list_debug(&self, separator: &str) -> String {
        self.clone()
            .into_iter()
            .map(|h| format!("{h:?}"))
            .join(separator)
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
        let last = headers[4];
        let mut tree = HeadersTree::new(10, &None);
        tree.insert_headers(&headers).unwrap();

        assert_eq!(tree.size(), 5);
        assert_eq!(tree.best_chain_tip(), &last);
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
        assert_eq!(tree.best_chain_tip(), &new_tip);

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
            tree.best_chain_fragment().list_to_string(",\n"),
            headers.into_iter().list_to_string(",\n"),
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
        assert_eq!(tree.best_chain_tip(), &new_tip);

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
    fn rollback_on_the_best_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        let middle = headers[3];
        let result = tree.select_rollback(&alice, &middle.hash()).unwrap();
        assert_eq!(result, RollbackTo(middle.hash()));
        assert_eq!(tree.best_chain_tip(), &headers[3]);
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
        assert_eq!(
            result,
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: headers[4]
            }
        );
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
        assert_eq!(
            result,
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: new_tip
            }
        );
    }

    #[test]
    fn rollback_beyond_limit() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let mut tree = initialize_with_peer(5, &alice);
        let headers = tree.best_chain_fragment();
        tree.initialize_peer(&bob, &headers[4].hash()).unwrap();
        let _ = rollforward_from(&mut tree, &headers[4], &alice, 15);

        // Bob tries to rollback on a header that doesn't exist in the tree anymore
        // Then bob is unknown
        let result = tree
            .select_rollback(&bob, &headers[3].hash())
            .err()
            .unwrap();
        assert_matches!(result, ConsensusError::UnknownPeer(p) if p == bob);
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
        assert_eq!(result, RollbackTo(rollback_point.hash()));
        assert_eq!(tree.best_chain_tip(), &headers[0]);
    }

    #[test]
    fn rollback_just_rolls_back_if_there_was_only_one_best_chain() {
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
        //     + - 5 - 6 - 7 - 6 - 9 (*bob)

        // sanity check: bob chain is the longest
        assert_eq!(tree.best_chain_tip(), added_headers.last().unwrap());

        // Now bob is rolled back 2 headers. The best chain stays at 9
        // but bob is rolled backed to 7
        // 0 - 1 - 2 - 3 - 4 (*alice)
        //     + - 5 - 6 - 7 (bob)
        let result = tree
            .select_rollback(&bob, &added_headers[1].hash())
            .unwrap();

        // This switches the fork back to alice at the intersection point of their chains
        let forked: Vec<TestHeader> = headers.split_off(2);
        let fork = Fork {
            peer: alice,
            rollback_point: headers[1].point(),
            fork: forked,
        };
        assert_eq!(result, RollbackChainSelection::SwitchToFork(fork));
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
        //     + - 5 - 6 - 7 - 8 - 9 (*bob)

        // sanity check: bob chain is the longest
        assert_eq!(tree.best_chain_tip(), added_headers.last().unwrap());

        // alice adds one more header. Now bob's chain is the best
        // and both bob and alice are in the best chains list
        //
        // 0 - 1 - 2 - 3 - 4 - 10 (alice)
        //     + - 5 - 6 - 7 - 8 - 9 (*bob)
        let alice_added_headers = rollforward_from(&mut tree, &headers[4], &alice, 1);

        // Now bob is rolled back 2 headers.
        // We internally switch to alice's chain as the best
        // 0 - 1 - 2 - 3 - 4 - 10 (*alice)
        //     + - 5 - 6 - 7 (bob)
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
    fn rollback_is_a_switch_even_if_we_roll_forward_again_on_previous_best_chain() {
        // In the case we end-up in this situation:
        //  alice has the best chain and charlie is about to roll to 3
        //  0 - 1 - 2 - 3  used to be bob's best chain
        //
        // 0 (bob)
        // + - 1
        //     + - 2  (charlie)
        //     |   + - 3
        //     + - 4 (*alice)
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

        let results = execute_json_actions(10, &actions).unwrap();
        assert_matches!(results.last(), Some(Forward(SwitchToFork(_))));
    }

    #[test]
    #[should_panic(
        expected = "Cannot create a headers tree with maximum chain length lower than 2"
    )]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<TestHeader>::new(1, &None);
    }

    const DEPTH: usize = 10;
    const MAX_LENGTH: usize = 5;
    const TEST_CASES_NB: u32 = 1000;

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(TEST_CASES_NB).with_seed(42).end())]
        #[test]
        fn run_chain_selection(actions in any_select_chains(DEPTH, MAX_LENGTH)) {
            let results = execute_actions(DEPTH, &actions).unwrap();
            let actual_chains = make_best_chains_from_results(&results);
            let expected_chains = make_best_chains_from_actions(&actions);
            for (i, (actual, expected)) in actual_chains.iter().zip(expected_chains).enumerate() {
                assert!(expected.contains(actual), "\nFor action {}, the actual chain\n{}\n\nis not contained in the best chains\n\n{}\n\n", i+1,
                    actual.list_to_string(", "), expected.lists_to_string(", ", "\n "));
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
