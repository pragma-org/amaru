use criterion::{criterion_group, criterion_main, Criterion};

fn validate_conway_block() {
    let test_block = include_bytes!("../tests/data/mainnet_blockheader_10817298.cbor");
    let test_vector = vec![(
        "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
        "c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b",
        "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada",
        25626202470912_u64,
        22586623335121436_u64,
    )];

    for (_pool_id_str, vrf_vkey_hash_str, epoch_nonce_str, numerator, denominator) in test_vector {
        let mock = amaru_ouroboros::consensus::test::MockLedgerState::new(
            vrf_vkey_hash_str,
            numerator,
            denominator,
        );

        let _ = amaru_ouroboros::consensus::test::validate_conway_block(
            mock,
            epoch_nonce_str,
            test_block,
        );
    }
}

fn benchmark_validate_conway_block(c: &mut Criterion) {
    c.bench_function("validate_conway_block", |b| b.iter(validate_conway_block));
}

criterion_group!(benches, benchmark_validate_conway_block);
criterion_main!(benches);
