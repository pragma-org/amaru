use pallas_crypto::hash::Hash;
use pallas_primitives::babbage;

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

    /// Block height of the header w.r.t genesis block
    fn block_height(&self) -> u64;
}

/// Concrete Conway-era compatible `Header` implementation.
///
/// There's no difference in headers' structure between Babbage
/// and Conway era. The idea is that we only keep concrete the header from
/// the latest era, and convert other headers on the fly when needed.
pub type ConwayHeader = babbage::HeaderBody;

impl Header for ConwayHeader {
    fn hash(&self) -> Hash<32> {
        todo!()
    }

    fn parent(&self) -> Option<Hash<32>> {
        todo!()
    }

    fn block_height(&self) -> u64 {
        todo!()
    }
}
