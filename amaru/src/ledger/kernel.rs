pub use pallas_crypto::hash::{Hash, Hasher};
pub use pallas_primitives::conway::{MintedBlock, TransactionInput, TransactionOutput};
pub type Point = pallas_network::miniprotocols::Point;

/// Get a 'Point' correspondin to a particular block
pub fn block_point(block: &MintedBlock<'_>) -> Point {
    Point::Specific(
        block.header.header_body.slot,
        Hasher::<256>::hash(block.header.raw_cbor()).to_vec(),
    )
}
