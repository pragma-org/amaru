use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use std::time::Duration;
#[allow(unused_must_use)]
use vrf_dalek::vrf10::{PublicKey10, SecretKey10, VrfProof10};
use vrf_dalek::vrf10_batchcompat::{BatchItem, BatchVerifier, VrfProof10BatchCompat};

fn vrf10(c: &mut Criterion) {
    let mut group = c.benchmark_group("VRF10");
    let alpha_string = [0u8; 23];
    let secret_key = SecretKey10::generate(&mut ChaCha20Rng::from_seed([0u8; 32]));
    let public_key = PublicKey10::from(&secret_key);

    let vrf_proof = VrfProof10::generate(&public_key, &secret_key, &alpha_string);
    group.bench_function("Generation", |b| {
        b.iter(|| {
            VrfProof10::generate(&public_key, &secret_key, &alpha_string);
        })
    });
    group.bench_function("Verification", |b| {
        b.iter(|| {
            vrf_proof
                .verify(&public_key, &alpha_string)
                .expect("Should pass.");
        })
    });
}

static SIZE_BATCHES: [usize; 6] = [32, 64, 128, 256, 512, 1024];
fn vrf10_batchcompat(c: &mut Criterion) {
    let mut group = c.benchmark_group("VRF10 Batch Compat");
    let alpha = vec![0u8; 32];
    let mut rng = ChaCha20Rng::from_seed([0u8; 32]);

    let secret_key = SecretKey10::generate(&mut rng);
    let public_key = PublicKey10::from(&secret_key);

    let vrf_proof = VrfProof10BatchCompat::generate(&public_key, &secret_key, &alpha);

    group.bench_function("Generation", |b| {
        b.iter(|| {
            VrfProof10BatchCompat::generate(&public_key, &secret_key, &alpha);
        })
    });
    group.bench_function("Single Verification", |b| {
        b.iter(|| {
            vrf_proof.verify(&public_key, &alpha).expect("Should work");
        })
    });

    for size in SIZE_BATCHES {
        let mut pks = Vec::with_capacity(size);
        let mut proofs = Vec::with_capacity(size);
        let mut outputs = Vec::with_capacity(size);
        // We generate `nr_proofs` valid proofs.
        for _ in 0..size {
            let secret_key = SecretKey10::generate(&mut rng);
            let public_key = PublicKey10::from(&secret_key);
            pks.push(public_key);

            let vrf_proof = VrfProof10BatchCompat::generate(&public_key, &secret_key, &alpha);
            outputs.push(vrf_proof.proof_to_hash());
            proofs.push(vrf_proof);
        }

        group.bench_with_input(
            BenchmarkId::new("Batch Verification (and insertion)", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let mut batch_verifier = BatchVerifier::new(size);

                    for (proof, (&pk, output)) in proofs.iter().zip(pks.iter().zip(outputs.iter()))
                    {
                        batch_verifier
                            .insert(BatchItem {
                                output: output.clone(),
                                proof: proof.clone(),
                                key: pk,
                                msg: alpha.clone(),
                            })
                            .expect("Should not fail");
                    }
                    batch_verifier.verify().expect("Should pass");
                })
            },
        );
    }
}

criterion_group!(name = benches;
                 config = Criterion::default().measurement_time(Duration::new(30, 0));
                 targets = vrf10, vrf10_batchcompat);
criterion_main!(benches);
