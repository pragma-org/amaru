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

use amaru_kernel::{BlockHeader, BlockHeight, Point, Tip, make_header, to_cbor};
use criterion::{Criterion, criterion_group, criterion_main};

fn kernel_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("Kernel Types CBOR");
    group.measurement_time(Duration::from_secs(5));

    let header = make_header(1_234_567, 1_234_567_890, None);
    let bh = BlockHeader::from(header);
    let tip = Tip::new(Point::Origin, BlockHeight::from(0));
    let point = Point::Origin;

    group.bench_function("BlockHeader", |b| b.iter(|| black_box(to_cbor(black_box(&bh)))));
    group.bench_function("Tip", |b| b.iter(|| black_box(to_cbor(black_box(&tip)))));
    group.bench_function("Point", |b| b.iter(|| black_box(to_cbor(black_box(&point)))));

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets = kernel_types
);
criterion_main!(benches);
