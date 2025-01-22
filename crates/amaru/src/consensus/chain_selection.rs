use super::header::Header;
use super::peer::Peer;
use pallas_crypto::hash::Hash;
use std::collections::HashMap;
use std::fmt::Debug;
use tracing::instrument;

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

impl<H: Header + Clone> Fragment<H> {
    fn start_from(tip: &H) -> Fragment<H> {
        Fragment {
            headers: vec![],
            anchor: tip.clone(),
        }
    }

    fn height(&self) -> u64 {
        self.headers.len() as u64 + self.anchor.block_height()
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

#[derive(Debug, PartialEq)]
pub enum ChainSelection<H: Header> {
    NewTip(H),
    RollbackTo(Hash<32>),
    NoChange,
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

    pub fn build(&self) -> ChainSelector<H> {
        ChainSelector {
            tip: self
                .tip
                .clone()
                .expect("cannot build a chain selector without a tip"),
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
        }
    }
}

impl<H: Header + Clone> Default for ChainSelectorBuilder<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> ChainSelector<H>
where
    H: Header + Clone + Debug,
{
    /// Roll forward the chain with a new header from given peer.
    ///
    /// The function returns the result of the chain selection process, which might lead
    /// to a new tip, no change, or some change in status for the peer.
    #[instrument(skip(self, header), fields(slot = header.slot(), hdr = header.hash().to_string()))]
    pub fn roll_forward(&mut self, peer: &Peer, header: H) -> ChainSelection<H> {
        let fragment = self.peers_chains.get_mut(peer).unwrap();
        let parent = header.parent().unwrap();
        let what_to_do = match fragment.headers.last() {
            Some(peer_tip) => {
                if peer_tip.hash() == parent {
                    ChainSelection::NewTip(header)
                } else {
                    ChainSelection::NoChange
                }
            }
            None => {
                if fragment.anchor.hash() == parent {
                    ChainSelection::NewTip(header)
                } else {
                    ChainSelection::NoChange
                }
            }
        };
        if let ChainSelection::NewTip(hdr) = what_to_do {
            // TODO: Avoid all those clones
            fragment.headers.push(hdr.clone());
            if fragment.height() > self.tip.block_height() {
                self.tip = hdr.clone();
                return ChainSelection::NewTip(self.tip.clone());
            }
        }
        ChainSelection::NoChange
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
    #[instrument(skip(self))]
    pub fn rollback(&mut self, peer: &Peer, point: Hash<32>) -> ChainSelection<H> {
        self.rollback_fragment(peer, point);

        let (best_peer, best_tip) = self.find_best_chain().unwrap();

        let result = if best_peer == *peer {
            ChainSelection::RollbackTo(point)
        } else {
            ChainSelection::NewTip(best_tip.clone())
        };

        self.tip = best_tip.clone();

        result
    }

    #[instrument(skip(self))]
    fn find_best_chain(&self) -> Option<(Peer, H)> {
        let mut best: Option<(Peer, H)> = None;
        for (peer, fragment) in self.peers_chains.iter() {
            let best_height = best.as_ref().map_or(0, |(_, tip)| tip.block_height());
            if fragment.height() >= best_height {
                best = Some((peer.clone(), fragment.tip()));
            }
        }
        best
    }

    #[instrument(skip(self))]
    fn rollback_fragment(&mut self, peer: &Peer, point: Hash<32>) {
        let fragment = self.peers_chains.get_mut(peer).unwrap();
        let rollback_point = fragment.position_of(point).map_or(0, |p| p + 1);
        fragment.headers.truncate(rollback_point);
    }
}

#[cfg(test)]
mod tests {

    use crate::consensus::header::{generate_headers_anchored_at, random_bytes, TestHeader};

    use super::ChainSelection::*;
    use super::*;

    #[test]
    fn extends_the_chain_with_single_header_from_peer() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        let result = chain_selector.roll_forward(&alice, header);

        assert_eq!(NewTip(header), result);
    }

    #[test]
    fn do_not_extend_the_chain_given_parent_does_not_match_tip() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

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

        chain_selector.roll_forward(&alice, header);
        let result = chain_selector.roll_forward(&alice, new_header);

        assert_eq!(NoChange, result);
    }

    #[test]
    #[should_panic]
    fn panic_when_forward_with_genesis_block() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        chain_selector.roll_forward(&alice, TestHeader::Genesis);
    }

    #[test]
    fn switch_to_fork_given_extension_is_longer_than_current_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");

        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&TestHeader::Genesis)
            .build();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain1.iter().for_each(|header| {
            chain_selector.roll_forward(&alice, *header);
        });

        let result = chain2
            .iter()
            .map(|header| chain_selector.roll_forward(&bob, *header))
            .last();

        assert_eq!(NewTip(*chain2.last().unwrap()), result.unwrap());
    }

    #[test]
    fn dont_switch_to_fork_given_extension_is_not_longer_than_current_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&TestHeader::Genesis)
            .build();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain2.iter().for_each(|header| {
            chain_selector.roll_forward(&bob, *header);
        });

        let result = chain1
            .iter()
            .map(|header| chain_selector.roll_forward(&alice, *header))
            .last();

        assert_eq!(NoChange, result.unwrap());
    }

    #[test]
    fn rollback_to_point_given_chain_is_still_longest() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);

        chain1.iter().for_each(|header| {
            chain_selector.roll_forward(&alice, *header);
        });

        let rollback_point = &chain1[3];
        let hash = rollback_point.hash();

        let result = chain_selector.rollback(&alice, hash);

        assert_eq!(rollback_point, chain_selector.best_chain());
        assert_eq!(RollbackTo(hash), result);
    }

    #[test]
    fn roll_forward_after_a_rollback() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 5);

        chain1.iter().for_each(|header| {
            chain_selector.roll_forward(&alice, *header);
        });

        let rollback_point = &chain1[2];
        let hash = rollback_point.hash();
        let new_header = TestHeader::TestHeader {
            block_number: (rollback_point.block_height() + 1) as u64,
            slot: 3,
            parent: rollback_point.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        chain_selector.rollback(&alice, hash);
        let result = chain_selector.roll_forward(&alice, new_header);

        assert_eq!(NewTip(new_header), result);
    }

    #[test]
    fn rollback_can_switch_chain_given_other_chain_is_longer() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .add_peer(&bob)
            .set_tip(&TestHeader::Genesis)
            .build();

        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 6);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain1.iter().for_each(|header| {
            chain_selector.roll_forward(&alice, *header);
        });

        chain2.iter().for_each(|header| {
            chain_selector.roll_forward(&bob, *header);
        });

        let rollback_point = &chain1[3];
        let result = chain_selector.rollback(&alice, rollback_point.hash());

        let expected_new_tip = chain2[5];
        assert_eq!(NewTip(expected_new_tip), result);
    }

    #[test]
    fn rollback_trims_whole_fragment_given_point_is_not_found() {
        let alice = Peer::new("alice");
        let mut chain_selector = ChainSelectorBuilder::new()
            .add_peer(&alice)
            .set_tip(&TestHeader::Genesis)
            .build();

        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        let result = chain_selector.rollback(&alice, header.hash());

        assert_eq!(RollbackTo(header.hash()), result);
    }
}
