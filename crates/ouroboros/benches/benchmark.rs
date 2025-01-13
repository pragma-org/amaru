use criterion::{criterion_group, criterion_main, Criterion};
use mockall::predicate::eq;
use ouroboros::consensus::BlockValidator;
use ouroboros::ledger::{MockLedgerState, PoolId, PoolSigma};
use ouroboros::validator::Validator;
use pallas_crypto::hash::Hash;
use pallas_math::math::FixedDecimal;
use pallas_traverse::MultiEraHeader;

fn validate_conway_block() {
    let test_block = include_bytes!("../tests/data/mainnet_blockheader_10817298.cbor");
    let test_vector = vec![(
        "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
        "c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b",
        "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada",
        25626202470912_u64,
        22586623335121436_u64,
        true,
    )];

    for (pool_id_str, vrf_vkey_hash_str, epoch_nonce_str, numerator, denominator, expected) in
        test_vector
    {
        let pool_id: PoolId = pool_id_str.parse().unwrap();
        let vrf_vkey_hash: Hash<32> = vrf_vkey_hash_str.parse().unwrap();
        let epoch_nonce: Hash<32> = epoch_nonce_str.parse().unwrap();

        let active_slots_coeff: FixedDecimal =
            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
        let conway_block_tag: u8 = 6;
        let multi_era_header = MultiEraHeader::decode(conway_block_tag, None, test_block).unwrap();
        let babbage_header = multi_era_header.as_babbage().expect("Infallible");
        assert_eq!(babbage_header.header_body.slot, 134402628u64);

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

        let block_validator = BlockValidator::new(
            babbage_header,
            &ledger_state,
            &epoch_nonce,
            &active_slots_coeff,
        );
        assert_eq!(block_validator.validate().is_ok(), expected);
    }
}

fn benchmark_validate_conway_block(c: &mut Criterion) {
    c.bench_function("validate_conway_block", |b| b.iter(validate_conway_block));
}

criterion_group!(benches, benchmark_validate_conway_block);
criterion_main!(benches);
