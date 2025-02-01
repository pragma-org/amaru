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

pub struct ChainSelectorBuilder<H> {
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

    pub fn set_tip(&mut self, new_tip: &H) {
        self.tip = Some(new_tip.clone());
    }

    pub fn add_peer(&mut self, peer: &Peer) {
        self.peers.push(peer.clone());
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
    /// Creates a new selector with some `tip` and following some `peers`.
    ///
    /// All the peers' fragments are anchored at the `tip` and initially
    /// empty.
    pub fn new(tip: H, peers: &[Peer]) -> ChainSelector<H> {
        let peers_chains: HashMap<Peer, Fragment<H>> = peers
            .iter()
            .map(|peer| (peer.clone(), Fragment::start_from(&tip)))
            .collect();

        ChainSelector { tip, peers_chains }
    }

    /// Roll forward the chain with a new header from given peer.
    ///
    /// The function returns the result of the chain selection process, which might lead
    /// to a new tip, no change, or some change in status for the peer.
    #[instrument(skip(self))]
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

    use super::ChainSelection::*;
    use super::*;

    use pallas_codec::minicbor as cbor;
    use pallas_crypto::hash::{Hash, Hasher};
    use rand::{rngs::StdRng, RngCore, SeedableRng};

    #[derive(Debug, PartialEq, Clone, Copy)]
    enum TestHeader {
        TestHeader {
            block_number: u64,
            slot: u64,
            parent: Hash<32>,
            body_hash: Hash<32>,
        },
        Genesis,
    }

    impl Header for TestHeader {
        fn genesis() -> Self {
            TestHeader::Genesis
        }

        fn hash(&self) -> pallas_crypto::hash::Hash<32> {
            self.hash()
        }

        fn parent(&self) -> Option<pallas_crypto::hash::Hash<32>> {
            match self {
                TestHeader::TestHeader { parent, .. } => Some(*parent),
                TestHeader::Genesis => None,
            }
        }

        fn block_height(&self) -> u64 {
            match self {
                TestHeader::TestHeader { block_number, .. } => *block_number,
                TestHeader::Genesis => 0,
            }
        }

        fn to_cbor(&self) -> Vec<u8> {
            let mut buffer = Vec::new();
            cbor::encode(self, &mut buffer)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
            buffer
        }

        fn from_cbor(bytes: &[u8]) -> Option<Self>
        where
            Self: Sized,
        {
            cbor::decode(bytes).ok()
        }
    }

    impl<C> cbor::encode::Encode<C> for TestHeader {
        fn encode<W: cbor::encode::Write>(
            &self,
            e: &mut cbor::Encoder<W>,
            ctx: &mut C,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            match self {
                TestHeader::TestHeader {
                    block_number,
                    slot,
                    parent,
                    body_hash,
                } => e
                    .encode(0)?
                    .array(4)?
                    .encode_with(block_number, ctx)?
                    .encode_with(slot, ctx)?
                    .encode_with(parent, ctx)?
                    .encode_with(body_hash, ctx)?
                    .ok(),
                TestHeader::Genesis => e.encode(1)?.ok(),
            }
        }
    }

    impl<'b, C> cbor::decode::Decode<'b, C> for TestHeader {
        fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            let tag = d.u8()?;
            match tag {
                0 => {
                    let block_number = d.decode_with(ctx)?;
                    let slot = d.decode_with(ctx)?;
                    let parent = d.decode_with(ctx)?;
                    let body_hash = d.decode_with(ctx)?;
                    Ok(TestHeader::TestHeader {
                        block_number,
                        slot,
                        parent,
                        body_hash,
                    })
                }
                1 => Ok(TestHeader::Genesis),
                _ => Err(cbor::decode::Error::message(format!("unknown tag {}", tag))),
            }
        }
    }

    impl TestHeader {
        fn hash(&self) -> Hash<32> {
            Hasher::<256>::hash(self.to_cbor().as_slice())
        }

        fn block_height(&self) -> u32 {
            match self {
                TestHeader::TestHeader { block_number, .. } => *block_number as u32,
                TestHeader::Genesis => 0,
            }
        }

        fn slot(&self) -> u32 {
            match self {
                TestHeader::TestHeader { slot, .. } => *slot as u32,
                TestHeader::Genesis => 0,
            }
        }
    }

    fn generate_headers_anchored_at(anchor: TestHeader, length: u32) -> Vec<TestHeader> {
        let mut headers: Vec<TestHeader> = Vec::new();
        for i in 0..length {
            let parent = if i == 0 {
                anchor.hash()
            } else {
                headers[i as usize - 1].hash()
            };
            let header = TestHeader::TestHeader {
                block_number: (i + anchor.block_height()) as u64,
                slot: (i + anchor.slot()) as u64,
                parent,
                body_hash: random_bytes(32).as_slice().into(),
            };
            headers.push(header);
        }
        headers
    }

    fn random_bytes(arg: u32) -> Vec<u8> {
        let mut rng = StdRng::from_entropy();
        let mut buffer = vec![0; arg as usize];
        rng.fill_bytes(&mut buffer);
        buffer
    }

    #[test]
    fn extends_the_chain_with_single_header_from_peer() {
        let alice = Peer::new("alice");
        let peers = [alice.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
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
        let peers = [alice.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
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
        let peers = [alice.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);

        chain_selector.roll_forward(&alice, TestHeader::Genesis);
    }

    #[test]
    fn switch_to_fork_given_extension_is_longer_than_current_chain() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let peers = [alice.clone(), bob.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
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
        let peers = [alice.clone(), bob.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
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
        let peers = [alice.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
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
        let peers = [alice.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
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
        let peers = [alice.clone(), bob.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);
        let chain1 = generate_headers_anchored_at(TestHeader::Genesis, 6);
        let chain2 = generate_headers_anchored_at(TestHeader::Genesis, 6);

        chain1.iter().for_each(|header| {
            chain_selector.roll_forward(&alice, *header);
        });

        chain2.iter().for_each(|header| {
            chain_selector.roll_forward(&bob, *header);
        });

        let rollback_point = &chain1[4];
        let result = chain_selector.rollback(&alice, rollback_point.hash());

        let expected_new_tip = chain2[5];
        assert_eq!(NewTip(expected_new_tip), result);
    }

    #[test]
    fn rollback_trims_whole_fragment_given_point_not_found() {
        let alice = Peer::new("alice");
        let peers = [alice.clone()];
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis, &peers);

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
