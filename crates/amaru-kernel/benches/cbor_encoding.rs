// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{hint::black_box, time::Duration};

use amaru_kernel::{
    MultiEraBlock, NetworkPoint, NetworkTip, ParsedBlockHeader, cardano::network_block::CONWAY_BLOCK, cbor,
    extract_block_header_cbor, make_header, to_cbor,
};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn kernel_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("Kernel Types CBOR");
    group.measurement_time(Duration::from_secs(5));

    let header = make_header(1_234_567, 1_234_567_890, None);
    let point = NetworkPoint::Origin;
    let tip = NetworkTip::origin();

    group.bench_function("Header", |b| b.iter(|| black_box(to_cbor(black_box(&header)))));
    group.bench_function("NetworkPoint", |b| b.iter(|| black_box(to_cbor(black_box(&point)))));
    group.bench_function("NetworkTip", |b| b.iter(|| black_box(to_cbor(black_box(&tip)))));

    group.finish();
}

fn header_extraction_baseline(input: &[u8]) -> Result<&[u8], cbor::decode::Error> {
    let mut decoder = cbor::Decoder::new(input);
    if decoder.array()?.is_some_and(|length| length != 2) {
        return Err(cbor::decode::Error::message("expected network block era and payload"));
    }
    let variant = decoder.u8()?;
    if decoder.array()? == Some(0) {
        return Err(cbor::decode::Error::message("missing block header"));
    }
    let (_, bytes) = cbor::tee(&mut decoder, cbor::skip_array)?;
    Ok(ParsedBlockHeader::from_cbor(variant, bytes)?.cbor())
}

#[expect(clippy::expect_used)]
fn block_parsing(c: &mut Criterion) {
    let mut encoder = cbor::Encoder::new(Vec::new());
    encoder.array(2).expect("encode wrapper").u8(7).expect("encode era").array(5).expect("encode block");
    encoder.writer_mut().extend_from_slice(header_extraction_baseline(&CONWAY_BLOCK).expect("extract fixture header"));
    for _ in 0..2 {
        encoder.array(20_000).expect("encode body array");
        for _ in 0..20_000 {
            encoder.null().expect("encode body item");
        }
    }
    encoder.map(0).expect("encode auxiliary data").array(0).expect("encode invalid transactions");
    let many_body_terms = encoder.into_writer();
    let fixtures: &[(&str, &[u8])] = &[
        ("conway", &CONWAY_BLOCK),
        (
            "large_fixture",
            include_bytes!(
                "../tests/data/cbor.decode/block/e1b90d83d6ae89860e2d1a0f398355cd4ed6defddb028dd610748d1f5610b546/sample.cbor"
            ),
        ),
        ("many_body_terms", &many_body_terms),
    ];
    let mut group = c.benchmark_group("Block parsing");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
    for (name, bytes) in fixtures {
        assert!(MultiEraBlock::decode(bytes).is_ok());
        assert_eq!(
            header_extraction_baseline(bytes).expect("extract baseline header"),
            extract_block_header_cbor(bytes).expect("extract header")
        );
        group.bench_with_input(BenchmarkId::new("header_baseline", name), bytes, |b, bytes| {
            b.iter(|| black_box(header_extraction_baseline(black_box(bytes))))
        });
        group.bench_with_input(BenchmarkId::new("header", name), bytes, |b, bytes| {
            b.iter(|| black_box(extract_block_header_cbor(black_box(bytes))))
        });
        group.bench_with_input(BenchmarkId::new("block", name), bytes, |b, bytes| {
            b.iter(|| black_box(MultiEraBlock::decode(black_box(bytes))))
        });
    }
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets = kernel_types, block_parsing
);
criterion_main!(benches);
