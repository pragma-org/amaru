use pallas_crypto::hash::{Hash, Hasher};
use pallas_traverse::update::RationalNumber;

pub type PoolId = Hash<28>;

/// The node's vkey is hashed with blake2b224 to create the pool id
pub fn issuer_vkey_to_pool_id(issuer_vkey: &[u8]) -> PoolId {
    Hasher::<224>::hash(issuer_vkey)
}

