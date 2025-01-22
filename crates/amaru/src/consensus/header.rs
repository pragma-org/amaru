use pallas_codec::minicbor;
use pallas_crypto::hash::Hash;
use pallas_network::miniprotocols::Point;
use pallas_primitives::babbage;
use pallas_traverse::ComputeHash;

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
        Hash::from(self.compute_hash().as_ref())
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
        minicbor::decode(bytes).ok()
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
