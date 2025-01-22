use mockall::automock;
use pallas_codec::utils::Bytes;
use pallas_crypto::hash::{Hash, Hasher};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("PoolId not found")]
    PoolIdNotFound,
}

pub type PoolId = Hash<28>;

/// The sigma value of a pool. This is a rational number that represents the total value of the
/// delegated stake in the pool over the total value of the active stake in the network. This value
/// is tracked in the ledger state and recorded as a snapshot value at each epoch.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PoolSigma {
    pub numerator: u64,
    pub denominator: u64,
}

/// The LedgerState trait provides a lookup mechanism for various information sourced from the ledger
#[automock]
pub trait LedgerState: Send + Sync {
    /// Performs a lookup of a pool_id to its sigma value. This usually represents a different set of
    /// sigma snapshot data depending on whether we need to look up the pool_id in the current epoch
    /// or in the future.
    fn pool_id_to_sigma(&self, pool_id: &PoolId) -> Result<PoolSigma, Error>;

    /// Hashes the vrf vkey of a pool.
    fn vrf_vkey_hash(&self, pool_id: &PoolId) -> Result<Hash<32>, Error>;

    /// Calculate the KES period given an absolute slot and some shelley-genesis values
    fn slot_to_kes_period(&self, slot: u64) -> u64;

    /// Get the maximum number of KES evolutions from the ledger state
    fn max_kes_evolutions(&self) -> u64;

    /// Get the latest opcert sequence number we've seen for a given issuer_vkey
    fn latest_opcert_sequence_number(&self, pool_id: &PoolId) -> Option<u64>;
}

/// The node's cold vkey is hashed with blake2b224 to create the pool id
pub fn issuer_vkey_to_pool_id(issuer_vkey: &Bytes) -> PoolId {
    Hasher::<224>::hash(issuer_vkey)
}

#[cfg(test)]
mod tests {
    use crate::ledger::issuer_vkey_to_pool_id;
    use pallas_codec::utils::Bytes;

    #[test]
    fn test_issuer_vkey_to_pool_id() {
        let test_vector = vec![(
            "cad3c900ca6baee9e65bf61073d900bfbca458eeca6d0b9f9931f5b1017a8cd6",
            "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
        )];
        insta::assert_yaml_snapshot!(test_vector);

        for (issuer_vkey_str, expected_pool_id_str) in test_vector {
            let issuer_vkey: Bytes = issuer_vkey_str.parse().unwrap();
            let pool_id = issuer_vkey_to_pool_id(&issuer_vkey);
            assert_eq!(pool_id.to_string(), expected_pool_id_str);
        }
    }
}
