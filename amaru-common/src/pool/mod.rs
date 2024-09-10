use pallas_crypto::hash::{Hash, Hasher};

pub type PoolId = Hash<28>;

/// The node's cold vkey is hashed with blake2b224 to create the pool id
pub fn issuer_vkey_to_pool_id(issuer_vkey: &[u8]) -> PoolId {
    Hasher::<224>::hash(issuer_vkey)
}

#[cfg(test)]
mod tests {
    #[test]
    fn issuer_vkey_to_pool_id() {
        let issuer_vkey =
            hex::decode("cad3c900ca6baee9e65bf61073d900bfbca458eeca6d0b9f9931f5b1017a8cd6")
                .unwrap();
        let pool_id = crate::pool::issuer_vkey_to_pool_id(&issuer_vkey);
        assert_eq!(
            pool_id.to_string(),
            "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114"
        );
    }
}
