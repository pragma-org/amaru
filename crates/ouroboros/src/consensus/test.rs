/// A module to expose functions for testing purpose
use crate::consensus::validator::BlockValidator;
use crate::{
    traits::{HasStakeDistribution, PoolSummary},
    validator::{ValidationError, Validator},
    Lovelace, PoolId, Slot, VrfKeyhash,
};
use pallas_codec::minicbor;
use pallas_crypto::hash::Hash;
use pallas_math::math::FixedDecimal;
use pallas_primitives::babbage;
use std::collections::HashMap;

pub fn validate_conway_block(
    mock_ledger_state: impl HasStakeDistribution,
    epoch_nonce_str: &str,
    test_block: &[u8],
) -> Result<(), ValidationError> {
    let epoch_nonce: Hash<32> = epoch_nonce_str.parse().unwrap();

    let active_slots_coeff: FixedDecimal = FixedDecimal::from(5u64) / FixedDecimal::from(100u64);

    let header: babbage::MintedHeader<'_> = minicbor::decode(test_block).unwrap();

    let block_validator = BlockValidator::new(
        &header,
        &mock_ledger_state,
        &epoch_nonce,
        &active_slots_coeff,
    );
    block_validator.validate()
}

pub struct MockLedgerState {
    pub vrf_vkey_hash: VrfKeyhash,
    pub stake: Lovelace,
    pub active_stake: Lovelace,
    pub op_certs: HashMap<PoolId, u64>,
    pub slots_per_kes_period: u64,
    pub max_kes_evolutions: u64,
}

impl MockLedgerState {
    pub fn new(vrf_vkey_hash: &str, stake: Lovelace, active_stake: Lovelace) -> Self {
        Self {
            vrf_vkey_hash: vrf_vkey_hash.parse().unwrap(),
            stake,
            active_stake,
            op_certs: Default::default(),
            slots_per_kes_period: 129600, // from shelley-genesis.json (1.5 days in seconds)
            max_kes_evolutions: 62,
        }
    }
}

impl HasStakeDistribution for MockLedgerState {
    fn get_pool(&self, _slot: Slot, _pool: &PoolId) -> Option<PoolSummary> {
        Some(PoolSummary {
            vrf: self.vrf_vkey_hash,
            stake: self.stake,
            active_stake: self.active_stake,
        })
    }

    fn slot_to_kes_period(&self, slot: u64) -> u64 {
        slot / self.slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        self.max_kes_evolutions
    }

    fn latest_opcert_sequence_number(&self, pool: &PoolId) -> Option<u64> {
        self.op_certs.get(pool).copied()
    }
}

#[cfg(test)]
pub mod tests {
    use super::validate_conway_block;
    use crate::validator::ValidationError;
    use ctor::ctor;
    use serde::{Deserialize, Serialize};

    #[ctor]
    fn init() {
        // set rust log level to TRACE
        // std::env::set_var("RUST_LOG", "trace");
        // initialize tracing crate
        // tracing_subscriber::fmt::init();
    }

    pub const TEST_BLOCK: &[u8; 859] =
        include_bytes!("../../tests/data/mainnet_blockheader_10817298.cbor");

    #[derive(Serialize, Deserialize)]
    struct TestSample<'a> {
        pool_id_str: &'a str,
        vrf_vkey_hash_str: &'a str,
        epoch_nonce_str: &'a str,
        numerator: u64,
        denominator: u64,
        expected: Result<(), ValidationError>,
    }

    #[test]
    fn test_validate_conway_block() {
        let test_block_hex = hex::encode(TEST_BLOCK);
        insta::assert_snapshot!(test_block_hex);
        let test_vector: Vec<TestSample> = vec![
            TestSample {
                pool_id_str: "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
                vrf_vkey_hash_str:
                    "c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b",
                epoch_nonce_str: "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada",
                numerator: 25626202470912_u64,
                denominator: 22586623335121436_u64,
                expected: Ok(()),
            },
            TestSample {
                pool_id_str: "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
                vrf_vkey_hash_str:
                    "c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b",
                epoch_nonce_str: "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada",
                numerator: 6026202470912_u64,
                denominator: 22586623335121436_u64,
                expected: Err(ValidationError::InsufficientPoolStake),
            },
        ];
        insta::assert_yaml_snapshot!(test_vector);

        for TestSample {
            pool_id_str: _,
            vrf_vkey_hash_str,
            epoch_nonce_str,
            numerator,
            denominator,
            expected,
        } in test_vector
        {
            let mock = super::MockLedgerState::new(vrf_vkey_hash_str, numerator, denominator);

            assert_eq!(
                validate_conway_block(mock, epoch_nonce_str, TEST_BLOCK,),
                expected
            );
        }
    }
}
