use std::{collections::HashMap, iter::Map};

use pallas_crypto::hash::Hash;

/// Interface to a header for the purpose of chain selection.
pub trait Header {
    /// Hash of the header
    ///
    /// This is used to identify the header in the chain selection.
    /// Header hash is expected to be unique for each header, eg.
    /// $h \neq h' \logeq hhash() \new h'.hash()$.
    fn hash(&self) -> Hash<32>;

    /// Parent hash of the header
    /// Not all headers have a parent, eg. genesis block.
    fn parent(&self) -> Option<Hash<32>>;
}

/// A single peer in the network, uniquely identified.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct Peer {
    name: String,
}

impl Peer {
    pub fn new(name: &str) -> Peer {
        Peer {
            name: name.to_string(),
        }
    }
}

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
    NoChange,
}

impl<H: Header + Clone> ChainSelector<H> {
    pub fn new(tip: H, peers: &[Peer]) -> ChainSelector<H> {
        let peers_chains: HashMap<Peer, Fragment<H>> = peers
            .iter()
            .map(|peer| (peer.clone(), Fragment::start_from(&tip)))
            .collect();

        ChainSelector { tip, peers_chains }
    }

    pub fn roll_forward(&mut self, peer: &Peer, header: H) -> ChainSelection<H> {
        match header.parent() {
            Some(parent) if parent != self.tip.hash() => ChainSelection::NoChange,
            None => panic!("genesis block is not expected to be rolled forward"),
            _ => {
                self.tip = header;
                ChainSelection::NewTip(self.tip.clone())
            }
        }
    }

    pub fn best_chain(&self) -> &H {
        &self.tip
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
        fn hash(&self) -> pallas_crypto::hash::Hash<32> {
            self.hash()
        }

        fn parent(&self) -> Option<pallas_crypto::hash::Hash<32>> {
            match self {
                TestHeader::TestHeader { parent, .. } => Some(*parent),
                TestHeader::Genesis => None,
            }
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
            Hasher::<256>::hash(self.cbor().as_slice())
        }

        fn cbor(&self) -> Vec<u8> {
            let mut buffer = Vec::new();
            cbor::encode(self, &mut buffer)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
            buffer
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
}
