// Copyright 2024 PRAGMA
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

use super::header::Header;
use crate::{ConsensusError, peer::Peer};
use amaru_kernel::Point;
use pallas_crypto::hash::Hash;
use std::{collections::HashMap, fmt::Debug};
use tracing::{Level, instrument};

/// A fragment of the chain, represented by a list of headers
/// and an anchor.
/// The list of headers /must/ be a sequence of headers such that
/// each element has the next one as parent. The anchor is the
/// parent of the last element of the sqeuence.
#[derive(Debug, PartialEq)]
pub struct Fragment<H: Header> {
    headers: Vec<H>,
    anchor: H,
}

enum FragmentExtension {
    Extend,
    Ignore,
}

impl<H: Header + Clone> Fragment<H> {
    fn start_from(tip: &H) -> Fragment<H> {
        Fragment {
            headers: vec![],
            anchor: tip.clone(),
        }
    }

    fn height(&self) -> u64 {
        self.tip().block_height()
    }

    fn position_of(&self, point: Hash<32>) -> Option<usize> {
        self.headers
            .iter()
            .position(|header| header.hash() == point)
    }

    fn tip(&self) -> H {
        match self.headers.last() {
            Some(header) => header.clone(),
            None => self.anchor.clone(),
        }
    }

    fn extend_with(&mut self, header: &H) -> FragmentExtension {
        if let Some(parent) = header.parent() {
            if parent == self.tip().hash() {
                self.headers.push(header.clone());
                return FragmentExtension::Extend;
            }
        }
        FragmentExtension::Ignore
    }
}

/// Current state of chain selection process
///
/// Chain selection is parameterised by the header type `H`, in
/// order to better decouple the internals of what's a header from
/// the selection logic
pub struct ChainSelector<H: Header> {
    tip: H,
    peers_chains: HashMap<Peer, Fragment<H>>,
}

/// Definition of a fork.
///
/// FIXME: The peer should not be needed here, as the fork should be
/// comprised of known blocks. It is only needed to download the blocks
/// we don't currently store.
#[derive(Debug, PartialEq)]
pub struct Fork<H: Header> {
    pub peer: Peer,
    pub rollback_point: Point,
    pub tip: H,
    pub fork: Vec<H>,
}

/// The outcome of the chain selection process in  case of
/// roll forward.
#[derive(Debug, PartialEq)]
pub enum ForwardChainSelection<H: Header> {
    /// The current best chain has been extended with a (single) new header.
    NewTip(H),

    /// The current best chain is unchanged.
    NoChange,

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),
}

/// The outcome of the chain selection process in case of rollback
#[derive(Debug, PartialEq)]
pub enum RollbackChainSelection<H: Header> {
    /// The current best chain has been rolled back to the given hash.
    RollbackTo(Hash<32>),

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),
}

/// Builder pattern for `ChainSelector`.
///
/// Allows incrementally adding information to build a
/// fully functional `ChainSelector`.
pub struct ChainSelectorBuilder<H: Header> {
    tip: Option<H>,
    peers: Vec<Peer>,
}

impl<H: Header + Clone> ChainSelectorBuilder<H> {
    pub fn new() -> ChainSelectorBuilder<H> {
        ChainSelectorBuilder {
            tip: None,
            peers: Vec::new(),
        }
    }

    pub fn set_tip(&mut self, new_tip: &H) -> &mut Self {
        self.tip = Some(new_tip.clone());
        self
    }

    pub fn add_peer(&mut self, peer: &Peer) -> &mut Self {
        self.peers.push(peer.clone());
        self
    }

    #[allow(clippy::unwrap_used)]
    pub fn build(&self) -> Result<ChainSelector<H>, ConsensusError> {
        Ok(ChainSelector {
            tip: self.tip.clone().ok_or(ConsensusError::MissingTip)?,
            peers_chains: self
                .peers
                .iter()
                .map(|peer| {
                    (
                        peer.clone(),
                        Fragment::start_from(self.tip.as_ref().unwrap()),
                    )
                })
                .collect(),
        })
    }
}

impl<H: Header + Clone> Default for ChainSelectorBuilder<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> ChainSelector<H>
where
    H: Header + Clone + Debug + PartialEq,
{
    /// Roll forward the chain with a new header from given peer.
    ///
    /// The function returns the result of the chain selection process, which might lead
    /// to a new tip, a switch to a fork, no change, or some change in status for the peer.
    #[instrument(level = Level::TRACE, skip_all,
                 fields(peer = peer.name,
                        header.slot = header.slot(),
                        header.hash = %header.hash()))]
    #[allow(clippy::unwrap_used)]
    pub fn select_roll_forward(&mut self, peer: &Peer, header: H) -> ForwardChainSelection<H> {
        use ForwardChainSelection::*;

        let fragment = self.peers_chains.get_mut(peer).unwrap();

        // TODO: raise error if header does not match parent
        match fragment.extend_with(&header) {
            FragmentExtension::Extend => {
                let (best_peer, best_tip) = self.find_best_chain().unwrap();

                let result = if best_tip.parent().unwrap() == self.tip.hash() {
                    NewTip(header.clone())
                } else if best_tip.block_height() > self.tip.block_height() {
                    let fragment = self.peers_chains.get(&best_peer).unwrap();
                    SwitchToFork(Fork {
                        peer: best_peer,
                        rollback_point: fragment.anchor.point(),
                        tip: best_tip,
                        fork: fragment.headers.clone(),
                    })
                } else {
                    NoChange
                };

                if result != NoChange {
                    self.tip = header;
                }

                result
            }
            _ => NoChange,
        }
    }

    pub fn best_chain(&self) -> &H {
        &self.tip
    }

    /// Rollback the chain to a given point.
    ///
    /// This function will rollback the chain of the given peer to the given point.
    /// If the chain of the peer is still the longest, the function will return a
    /// `RollbackTo` result, otherwise it will return a `NewTip` result with the new
    /// tip of the chain.
    #[instrument(level = Level::TRACE, skip(self), fields(peer = peer.name, %point))]
    #[allow(clippy::unwrap_used)]
    pub fn select_rollback(&mut self, peer: &Peer, point: Hash<32>) -> RollbackChainSelection<H> {
        use RollbackChainSelection::*;

        self.rollback_fragment(peer, point);

        let (best_peer, best_tip) = self.find_best_chain().unwrap();

        let result = if best_peer == *peer {
            RollbackTo(point)
        } else {
            let fragment = self.peers_chains.get(&best_peer).unwrap();
            // TODO: do not always switch to anchor if there's a better intersection
            // with current chain
            SwitchToFork(Fork {
                peer: best_peer,
                rollback_point: fragment.anchor.point(),
                tip: best_tip.clone(),
                fork: fragment.headers.clone(),
            })
        };

        self.tip = best_tip;

        result
    }

    #[instrument(level = Level::TRACE, skip(self))]
    fn find_best_chain(&self) -> Option<(Peer, H)> {
        let mut best: Option<(Peer, H)> = None;
        for (peer, fragment) in self.peers_chains.iter() {
            let best_height = best.as_ref().map_or(0, |(_, tip)| tip.block_height());
            if fragment.height() > best_height {
                best = Some((peer.clone(), fragment.tip()));
            }
        }
        best
    }

    #[allow(clippy::unwrap_used)]
    fn rollback_fragment(&mut self, peer: &Peer, point: Hash<32>) {
        let fragment = self.peers_chains.get_mut(peer).unwrap();
        let rollback_point = fragment.position_of(point).map_or(0, |p| p + 1);
        fragment.headers.truncate(rollback_point);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::consensus::header::test::{TestHeader, generate_headers_anchored_at, random_bytes};

    #[test]
    fn extends_the_chain_with_single_header_from_peer() {
        let alice = Peer::new("alice");
        let chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        let result = chain_selector.unwrap().select_roll_forward(&alice, header);

        assert_eq!(ForwardChainSelection::NewTip(header), result);
    }

    #[test]
    fn do_not_extend_the_chain_given_parent_does_not_match_tip() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build()
            .unwrap();

        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };
        let new_header = TestHeader::TestHeader {
            block_number: 1,
            slot: 1,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        chain_selector.select_roll_forward(&alice, header);
        let result = chain_selector.select_roll_forward(&alice, new_header);

        assert_eq!(ForwardChainSelection::NoChange, result);
    }

    #[test]
    fn dont_change_when_forward_with_genesis_block() {
        let alice = Peer::new("alice");
        let chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        let result = chain_selector
            .unwrap()
            .select_roll_forward(&alice, TestHeader::Genesis);

        assert_eq!(ForwardChainSelection::NoChange, result);
    }

    #[test]
    fn switch_to_fork_given_extension_is_longer_than_current_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&TestHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain1.iter().for_each(|header| {
            chain_selector.select_roll_forward(&alice, *header);
        });

        let result = chain2
            .iter()
            .map(|header| chain_selector.select_roll_forward(&bob, *header))
            .last();

        assert_eq!(
            ForwardChainSelection::SwitchToFork(Fork {
                peer: bob,
                rollback_point: Point::Origin,
                tip: chain2[5],
                fork: chain2
            }),
            result.unwrap()
        );
    }

    #[test]
    fn dont_switch_to_fork_given_extension_is_not_longer_than_current_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&TestHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain2.iter().for_each(|header| {
            chain_selector.select_roll_forward(&bob, *header);
        });

        let result = chain1
            .iter()
            .map(|header| chain_selector.select_roll_forward(&alice, *header))
            .last();

        assert_eq!(ForwardChainSelection::NoChange, result.unwrap());
    }

    #[test]
    fn rollback_to_point_given_chain_is_still_longest() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);

        chain1.iter().for_each(|header| {
            chain_selector.select_roll_forward(&alice, *header);
        });

        let rollback_point = &chain1[3];
        let hash = rollback_point.hash();

        let result = chain_selector.select_rollback(&alice, hash);

        assert_eq!(rollback_point, chain_selector.best_chain());
        assert_eq!(RollbackChainSelection::RollbackTo(hash), result);
    }

    #[test]
    fn roll_forward_after_a_rollback() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);

        chain1.iter().for_each(|header| {
            chain_selector.select_roll_forward(&alice, *header);
        });

        let rollback_point = &chain1[2];
        let hash = rollback_point.hash();
        let new_header = TestHeader::TestHeader {
            block_number: (rollback_point.block_height() + 1) as u64,
            slot: 3,
            parent: rollback_point.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        chain_selector.select_rollback(&alice, hash);
        let result = chain_selector.select_roll_forward(&alice, new_header);

        assert_eq!(ForwardChainSelection::NewTip(new_header), result);
    }

    #[test]
    fn rollback_can_switch_chain_given_other_chain_is_longer() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&TestHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 6);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain1.iter().for_each(|header| {
            chain_selector.select_roll_forward(&alice, *header);
        });

        chain2.iter().for_each(|header| {
            chain_selector.select_roll_forward(&bob, *header);
        });

        let rollback_point = &chain1[3];
        let result = chain_selector.select_rollback(&alice, rollback_point.hash());

        assert_eq!(
            RollbackChainSelection::SwitchToFork(Fork {
                peer: bob,
                rollback_point: Point::Origin,
                tip: chain2[5],
                fork: chain2
            }),
            result
        );
    }
}
