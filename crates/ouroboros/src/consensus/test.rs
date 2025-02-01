/// A module to expose functions for testing purpose
use crate::consensus::validator::BlockValidator;
use crate::ledger::{MockLedgerState, PoolSigma};
use crate::validator::Validator;
use crate::{ledger::PoolId, validator::ValidationError};
use mockall::predicate::eq;
use pallas_crypto::hash::Hash;
use pallas_math::math::FixedDecimal;
use pallas_primitives::conway::Header;
use pallas_traverse::MultiEraHeader;

pub fn validate_conway_block(
    pool_id_str: &str,
    vrf_vkey_hash_str: &str,
    epoch_nonce_str: &str,
    numerator: u64,
    denominator: u64,
    test_block: &[u8],
) -> Result<(), ValidationError> {
    let pool_id: PoolId = pool_id_str.parse().unwrap();
    let vrf_vkey_hash: Hash<32> = vrf_vkey_hash_str.parse().unwrap();
    let epoch_nonce: Hash<32> = epoch_nonce_str.parse().unwrap();

    let active_slots_coeff: FixedDecimal = FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
    let conway_block_tag: u8 = 6;
    let multi_era_header = MultiEraHeader::decode(conway_block_tag, None, test_block).unwrap();
    let babbage_header = multi_era_header.as_babbage().expect("Infallible");

    let mut ledger_state = MockLedgerState::new();
    ledger_state
        .expect_pool_id_to_sigma()
        .with(eq(pool_id))
        .returning(move |_| {
            Ok(PoolSigma {
                numerator,
                denominator,
            })
        });
    ledger_state
        .expect_vrf_vkey_hash()
        .with(eq(pool_id))
        .returning(move |_| Ok(vrf_vkey_hash));
    ledger_state.expect_slot_to_kes_period().returning(|slot| {
        // hardcode some values from shelley-genesis.json for the mock implementation
        let slots_per_kes_period: u64 = 129600; // from shelley-genesis.json (1.5 days in seconds)
        slot / slots_per_kes_period
    });
    ledger_state.expect_max_kes_evolutions().returning(|| 62);
    ledger_state
        .expect_latest_opcert_sequence_number()
        .returning(|_| None);

    let pseudo_header = Header::from(babbage_header.clone());
    let cbor = babbage_header.header_body.raw_cbor();
    let block_validator = BlockValidator::new(
        &pseudo_header,
        cbor,
        &ledger_state,
        &epoch_nonce,
        &active_slots_coeff,
    );
    block_validator.validate()
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
            pool_id_str,
            vrf_vkey_hash_str,
            epoch_nonce_str,
            numerator,
            denominator,
            expected,
        } in test_vector
        {
            assert_eq!(
                validate_conway_block(
                    pool_id_str,
                    vrf_vkey_hash_str,
                    epoch_nonce_str,
                    numerator,
                    denominator,
                    TEST_BLOCK,
                ),
                expected
            );
        }
    }
}
