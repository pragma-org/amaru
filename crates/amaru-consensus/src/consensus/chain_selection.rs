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

use crate::{peer::Peer, ConsensusError};
use amaru_kernel::{cbor, Point};
use amaru_ouroboros::HASH_SIZE;
use amaru_ouroboros_traits::is_header::IsHeader;
use pallas_crypto::hash::Hash;
use std::{collections::HashMap, fmt::Debug};
use tracing::{instrument, Level};

/// A fragment of the chain, represented by a list of headers
/// and an anchor.
/// The list of headers /must/ be a sequence of headers such that
/// each element has the next one as parent. The anchor is the
/// parent of the last element of the sequence.
#[derive(Debug, PartialEq)]
pub struct Fragment<H: IsHeader> {
    headers: Vec<H>,
    anchor: Tip<H>,
}

enum FragmentExtension {
    Extend,
    Ignore,
}

impl<H: IsHeader + Clone> Fragment<H> {
    fn start_from(tip: &Tip<H>) -> Fragment<H> {
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

    fn tip(&self) -> Tip<H> {
        match self.headers.last() {
            Some(header) => Tip::Hdr(header.clone()),
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

#[derive(Debug, PartialEq, Clone)]
pub enum Tip<H: IsHeader> {
    Genesis,
    Hdr(H),
}

impl<H, C> cbor::encode::Encode<C> for Tip<H>
where
    H: cbor::encode::Encode<C> + IsHeader,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Tip::Genesis => e.encode(0).map(|_| ()),
            Tip::Hdr(header) => header.encode(e, ctx),
        }
    }
}

impl<H: IsHeader> IsHeader for Tip<H> {
    fn parent(&self) -> Option<Hash<HASH_SIZE>> {
        match self {
            Tip::Genesis => None,
            Tip::Hdr(header) => header.parent(),
        }
    }

    fn block_height(&self) -> u64 {
        match self {
            Tip::Genesis => 0,
            Tip::Hdr(header) => header.block_height(),
        }
    }

    fn slot(&self) -> u64 {
        match self {
            Tip::Genesis => 0,
            Tip::Hdr(header) => header.slot(),
        }
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        match self {
            Tip::Genesis => vec![],
            Tip::Hdr(header) => header.extended_vrf_nonce_output(),
        }
    }
}

impl<H: IsHeader> From<Option<H>> for Tip<H> {
    fn from(tip: Option<H>) -> Tip<H> {
        match tip {
            Some(header) => Tip::Hdr(header),
            None => Tip::Genesis,
        }
    }
}
/// Current state of chain selection process
///
/// Chain selection is parameterised by the header type `H`, in
/// order to better decouple the internals of what's a header from
/// the selection logic
pub struct ChainSelector<H: IsHeader> {
    tip: Tip<H>,
    peers_chains: HashMap<Peer, Fragment<H>>,
}

/// Definition of a fork.
///
/// FIXME: The peer should not be needed here, as the fork should be
/// comprised of known blocks. It is only needed to download the blocks
/// we don't currently store.
#[derive(Debug, PartialEq)]
pub struct Fork<H: IsHeader> {
    pub peer: Peer,
    pub rollback_point: Point,
    pub tip: H,
    pub fork: Vec<H>,
}

/// The outcome of the chain selection process in  case of
/// roll forward.
#[derive(Debug, PartialEq)]
pub enum ForwardChainSelection<H: IsHeader> {
    /// The current best chain has been extended with a (single) new header.
    NewTip(H),

    /// The current best chain is unchanged.
    NoChange,

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),
}

/// The outcome of the chain selection process in case of rollback
#[derive(Debug, PartialEq)]
pub enum RollbackChainSelection<H: IsHeader> {
    /// The current best chain has been rolled back to the given hash.
    RollbackTo(Hash<32>),

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),
}

/// Builder pattern for `ChainSelector`.
///
/// Allows incrementally adding information to build a
/// fully functional `ChainSelector`.
pub struct ChainSelectorBuilder<H: IsHeader> {
    tip: Option<H>,
    peers: Vec<Peer>,
}

impl<H: IsHeader + Clone> ChainSelectorBuilder<H> {
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
            tip: self.tip.clone().into(),
            peers_chains: self
                .peers
                .iter()
                .map(|peer| {
                    (
                        peer.clone(),
                        Fragment::start_from(&(self.tip.clone().into())),
                    )
                })
                .collect(),
        })
    }
}

impl<H: IsHeader + Clone> Default for ChainSelectorBuilder<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> ChainSelector<H>
where
    H: IsHeader + Clone + Debug + PartialEq,
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
                    self.tip = Tip::Hdr(header);
                }

                result
            }
            _ => NoChange,
        }
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

        self.tip = Tip::Hdr(best_tip);

        result
    }

    #[instrument(level = Level::TRACE, skip(self))]
    fn find_best_chain(&self) -> Option<(Peer, H)> {
        let mut best: Option<(Peer, H)> = None;
        for (peer, fragment) in self.peers_chains.iter() {
            let best_height = best.as_ref().map_or(0, |(_, tip)| tip.block_height());
            if fragment.height() > best_height {
                // FIXME: height is necessarily greater than 0, therefore tip
                // can only be a header and not Genesis. How can I statically
                // enforce this?
                match fragment.tip() {
                    Tip::Hdr(header) => {
                        best = Some((peer.clone(), header.clone()));
                    }
                    #[allow(clippy::panic)]
                    Tip::Genesis => panic!("Fragment has no tip"),
                }
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
pub(crate) mod tests {
    use super::*;
    use amaru_kernel::{from_cbor, to_cbor};
    use amaru_ouroboros_traits::is_header::fake::FakeHeader;
    use proptest::prelude::*;
    use rand::{rngs::StdRng, RngCore, SeedableRng};

    /// Very simple function to generate random sequence of bytes of given length.
    pub fn random_bytes(arg: u32) -> Vec<u8> {
        let mut rng = StdRng::from_entropy();
        let mut buffer = vec![0; arg as usize];
        rng.fill_bytes(&mut buffer);
        buffer
    }

    /// Generate a chain of headers anchored at a given header.
    ///
    /// The chain is generated by creating headers with random body hash, and linking
    /// them to the previous header in the chain until the desired length is reached.
    pub fn generate_headers_anchored_at(anchor: FakeHeader, length: u32) -> Vec<FakeHeader> {
        let mut headers: Vec<FakeHeader> = Vec::new();
        let mut parent = anchor;
        for i in 0..u64::from(length) {
            let header = FakeHeader::FakeHeader {
                block_number: i + anchor.block_height() + 1,
                slot: i + anchor.slot() + 1,
                parent: parent.hash(),
                body_hash: random_bytes(32).as_slice().into(),
            };
            headers.push(header);
            parent = header;
        }
        headers
    }

    prop_compose! {
        fn any_test_header()(
            slot in 0..1000000u64,
            block_number in 0..100000u64,
            parent in any::<[u8; 32]>(),
            body in any::<[u8; 32]>(),
        )
            -> FakeHeader {
            FakeHeader::FakeHeader {
                block_number,
                slot,
                parent: parent.into(),
                body_hash: body.into(),
            }
        }
    }

    proptest! {
        #[test]
        fn prop_roundtrip_cbor(hdr in any_test_header()) {
            let bytes = to_cbor(&hdr);
            let hdr2 = from_cbor::<FakeHeader>(&bytes).unwrap();
            assert_eq!(hdr, hdr2);
        }
    }

    #[test]
    fn extends_the_chain_with_single_header_from_peer() {
        let alice = Peer::new("alice");
        let chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&FakeHeader::Genesis)
            .build();

        let header = FakeHeader::FakeHeader {
            block_number: 1,
            slot: 0,
            parent: FakeHeader::Genesis.hash(),
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
            .set_tip(&FakeHeader::Genesis)
            .build()
            .unwrap();

        let header = FakeHeader::FakeHeader {
            block_number: 1,
            slot: 0,
            parent: FakeHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };
        let new_header = FakeHeader::FakeHeader {
            block_number: 1,
            slot: 1,
            parent: FakeHeader::Genesis.hash(),
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
            .set_tip(&FakeHeader::Genesis)
            .build();

        let result = chain_selector
            .unwrap()
            .select_roll_forward(&alice, FakeHeader::Genesis);

        assert_eq!(ForwardChainSelection::NoChange, result);
    }

    #[test]
    fn switch_to_fork_given_extension_is_longer_than_current_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&FakeHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(FakeHeader::Genesis, 5);
        let chain2 = generate_headers_anchored_at(FakeHeader::Genesis, 6);

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
            .set_tip(&FakeHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(FakeHeader::Genesis, 5);
        let chain2 = generate_headers_anchored_at(FakeHeader::Genesis, 6);

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
            .set_tip(&FakeHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(FakeHeader::Genesis, 5);

        chain1.iter().for_each(|header| {
            chain_selector.select_roll_forward(&alice, *header);
        });

        let rollback_point = &chain1[3];
        let hash = rollback_point.hash();

        let result = chain_selector.select_rollback(&alice, hash);

        assert_eq!(Tip::Hdr(*rollback_point), chain_selector.tip);
        assert_eq!(RollbackChainSelection::RollbackTo(hash), result);
    }

    #[test]
    fn roll_forward_after_a_rollback() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&FakeHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(FakeHeader::Genesis, 5);

        chain1.iter().for_each(|header| {
            chain_selector.select_roll_forward(&alice, *header);
        });

        let rollback_point = &chain1[2];
        let hash = rollback_point.hash();
        let new_header = FakeHeader::FakeHeader {
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
            .set_tip(&FakeHeader::Genesis)
            .build()
            .unwrap();

        let chain1 = generate_headers_anchored_at(FakeHeader::Genesis, 6);
        let chain2 = generate_headers_anchored_at(FakeHeader::Genesis, 6);

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
