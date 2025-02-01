use pallas_codec::minicbor;
use pallas_codec::minicbor as cbor;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_network::miniprotocols::Point;
use pallas_primitives::babbage;
use pallas_traverse::ComputeHash;
use rand::{rngs::StdRng, RngCore, SeedableRng};

/// Interface to a header for the purpose of chain selection.
pub trait Header {
    /// Genesis header.
    fn genesis() -> Self;

    /// Hash of the header
    ///
    /// This is used to identify the header in the chain selection.
    /// Header hash is expected to be unique for each header, eg.
    /// $h \neq h' \logeq hhash() \new h'.hash()$.
    fn hash(&self) -> Hash<32>;

    /// Parent hash of the header
    /// Not all headers have a parent, eg. genesis block.
    fn parent(&self) -> Option<Hash<32>>;

    /// Block height of the header w.r.t genesis block
    fn block_height(&self) -> u64;

    /// Slot number of the header
    fn slot(&self) -> u64;

    /// Encode to CBOR
    fn to_cbor(&self) -> Vec<u8>;

    /// Decode from CBOR
    fn from_cbor(bytes: &[u8]) -> Option<Self>
    where
        Self: Sized;
}

/// Concrete Conway-era compatible `Header` implementation.
///
/// There's no difference in headers' structure between Babbage
/// and Conway era. The idea is that we only keep concrete the header from
/// the latest era, and convert other headers on the fly when needed.
pub type ConwayHeader = babbage::Header;

impl Header for ConwayHeader {
    fn genesis() -> Self {
        todo!()
    }

    fn hash(&self) -> Hash<32> {
        self.compute_hash()
    }

    fn parent(&self) -> Option<Hash<32>> {
        self.header_body.prev_hash
    }

    fn block_height(&self) -> u64 {
        self.header_body.block_number
    }

    fn to_cbor(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        minicbor::encode(self, &mut buffer)
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        buffer
    }

    fn from_cbor(bytes: &[u8]) -> Option<ConwayHeader> {
        minicbor::decode(bytes)
            .map_err(|e| {
                panic!("unable to decode value from CBOR: {e:?}");
            })
            .ok()
    }

    fn slot(&self) -> u64 {
        self.header_body.slot
    }
}

/// Utility function to retrieve the hash of a `Point`.
/// By convention, the hash of `Genesis` is all 0s.
pub fn point_hash(point: &Point) -> Hash<32> {
    match point {
        Point::Origin => Hash::from([0; 32]),
        Point::Specific(_, header_hash) => Hash::from(header_hash.as_ref()),
    }
}

/// Basic `Header` implementation for testing purposes.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TestHeader {
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
        cbor::decode(bytes)
            .map_err(|e| panic!("unable to encode value to CBOR: {e:?}"))
            .ok()
    }

    fn slot(&self) -> u64 {
        match self {
            TestHeader::TestHeader { slot, .. } => *slot,
            TestHeader::Genesis => 0,
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
                .end()?
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
                d.array()?;
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

/// Generate a chain of headers anchored at a given header.
///
/// The chain is generated by creating headers with random body hash, and linking
/// them to the previous header in the chain until the desired length is reached.
pub fn generate_headers_anchored_at(anchor: TestHeader, length: u32) -> Vec<TestHeader> {
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

/// Very simple function to generate random sequence of bytes of given length.
pub fn random_bytes(arg: u32) -> Vec<u8> {
    let mut rng = StdRng::from_entropy();
    let mut buffer = vec![0; arg as usize];
    rng.fill_bytes(&mut buffer);
    buffer
}

#[cfg(test)]
mod test {
    use super::*;
    use proptest::prelude::*;

    prop_compose! {
        fn any_test_header()(
            slot in 0..1000000u64,
            block_number in 0..100000u64,
            parent in any::<[u8; 32]>(),
            body in any::<[u8; 32]>(),
        )
            -> TestHeader {
            TestHeader::TestHeader {
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
            let bytes = hdr.to_cbor();
            let hdr2 = TestHeader::from_cbor(&bytes).unwrap();
            assert_eq!(hdr, hdr2);
        }
    }
}
