use ouroboros::ledger::{Error, LedgerState, PoolId, PoolSigma};
use pallas_crypto::hash::Hash;
use std::collections::HashMap;

#[derive(Debug)]
pub struct MockLedgerState {
    pool_id_to_vrf: HashMap<PoolId, Hash<32>>,
    pool_id_to_sigma: HashMap<PoolId, PoolSigma>,
}

/// Mock the ledger state for the block validator. These values are the happy path
/// and don't really validate anything currently. We assume the pool has plenty of stake, the
/// VRF key hash is always available.
pub fn new(
    pool_id_to_vrf: HashMap<PoolId, Hash<32>>,
    pool_id_to_sigma: HashMap<PoolId, PoolSigma>,
) -> MockLedgerState {
    MockLedgerState {
        pool_id_to_vrf,
        pool_id_to_sigma,
    }
}

impl LedgerState for MockLedgerState {
    fn pool_id_to_sigma(&self, pool_id: &PoolId) -> Result<PoolSigma, Error> {
        self.pool_id_to_sigma
            .get(pool_id)
            .cloned()
            .ok_or(Error::PoolIdNotFound)
    }

    fn vrf_vkey_hash(&self, pool_id: &PoolId) -> Result<Hash<32>, Error> {
        self.pool_id_to_vrf
            .get(pool_id)
            .cloned()
            .ok_or(Error::PoolIdNotFound)
    }

    fn slot_to_kes_period(&self, slot: u64) -> u64 {
        let slots_per_kes_period: u64 = 129600; // from shelley-genesis.json (1.5 days in seconds)
        slot / slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        62
    }

    fn latest_opcert_sequence_number(&self, _issuer_vkey: &[u8]) -> Option<u64> {
        None
    }
}
