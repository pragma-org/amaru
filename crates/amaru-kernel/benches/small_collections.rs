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

//! Container-level benchmarks for the small-collection sizes that dominate Amaru's per-block
//! state (mainnet blocks rarely put more than a handful of entries in any non-UTxO collection).
//! The alloc profiler reports allocation counts alongside timings, which is the primary metric
//! for compact-collection work: below the promotion threshold, the compact representation should
//! show at most one right-sized allocation. Sizes bracket the promotion threshold, plus larger
//! sizes to catch regressions in the tree-backed regime.

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{CompactMap, CompactSet};
use divan::{Bencher, black_box};
use rand::{Rng, RngCore, SeedableRng, rngs::StdRng};

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    divan::main();
}

/// Same size as a stake credential or pool id hash, the dominant key shape in the volatile state.
type Key = [u8; 28];

const SIZES: &[usize] = &[0, 1, 2, 3, 4, 5, 8, 16, 64];

/// Promotion threshold for the compact variants: sizes 0..=5 stay in the small regime, 8/16/64
/// exercise the promoted (tree-backed) regime.
const SMALL: usize = 5;

const SEED: u64 = 0xA11C_E00B;

fn keys(n: usize) -> Vec<Key> {
    let mut rng = StdRng::seed_from_u64(SEED);
    (0..n)
        .map(|_| {
            let mut key = Key::default();
            rng.fill_bytes(&mut key);
            key
        })
        .collect()
}

#[divan::bench(args = SIZES)]
fn btree_set_build(bencher: Bencher<'_, '_>, n: usize) {
    let keys = keys(n);
    bencher.bench(|| black_box(&keys).iter().copied().collect::<BTreeSet<Key>>());
}

#[divan::bench(args = SIZES)]
fn btree_set_clone(bencher: Bencher<'_, '_>, n: usize) {
    let set = keys(n).into_iter().collect::<BTreeSet<Key>>();
    bencher.bench(|| black_box(&set).clone());
}

#[divan::bench(args = SIZES)]
fn btree_set_contains(bencher: Bencher<'_, '_>, n: usize) {
    let keys = keys(n);
    let set = keys.iter().copied().collect::<BTreeSet<Key>>();
    bencher.bench(|| black_box(&keys).iter().filter(|key| black_box(&set).contains(*key)).count());
}

#[divan::bench(args = SIZES)]
fn btree_set_iter(bencher: Bencher<'_, '_>, n: usize) {
    let set = keys(n).into_iter().collect::<BTreeSet<Key>>();
    bencher.bench(|| black_box(&set).iter().count());
}

#[divan::bench(args = SIZES)]
fn compact_set_build(bencher: Bencher<'_, '_>, n: usize) {
    let keys = keys(n);
    bencher.bench(|| black_box(&keys).iter().copied().collect::<CompactSet<Key, SMALL>>());
}

#[divan::bench(args = SIZES)]
fn compact_set_clone(bencher: Bencher<'_, '_>, n: usize) {
    let set = keys(n).into_iter().collect::<CompactSet<Key, SMALL>>();
    bencher.bench(|| black_box(&set).clone());
}

#[divan::bench(args = SIZES)]
fn compact_set_contains(bencher: Bencher<'_, '_>, n: usize) {
    let keys = keys(n);
    let set = keys.iter().copied().collect::<CompactSet<Key, SMALL>>();
    bencher.bench(|| black_box(&keys).iter().filter(|key| black_box(&set).contains(*key)).count());
}

#[divan::bench(args = SIZES)]
fn compact_set_iter(bencher: Bencher<'_, '_>, n: usize) {
    let set = keys(n).into_iter().collect::<CompactSet<Key, SMALL>>();
    bencher.bench(|| black_box(&set).iter().count());
}

fn entries(n: usize) -> Vec<(Key, u64)> {
    let mut rng = StdRng::seed_from_u64(SEED);
    keys(n).into_iter().map(|key| (key, rng.random())).collect()
}

#[divan::bench(args = SIZES)]
fn btree_map_build(bencher: Bencher<'_, '_>, n: usize) {
    let entries = entries(n);
    bencher.bench(|| black_box(&entries).iter().copied().collect::<BTreeMap<Key, u64>>());
}

#[divan::bench(args = SIZES)]
fn btree_map_clone(bencher: Bencher<'_, '_>, n: usize) {
    let map = entries(n).into_iter().collect::<BTreeMap<Key, u64>>();
    bencher.bench(|| black_box(&map).clone());
}

#[divan::bench(args = SIZES)]
fn btree_map_get(bencher: Bencher<'_, '_>, n: usize) {
    let entries = entries(n);
    let map = entries.iter().copied().collect::<BTreeMap<Key, u64>>();
    bencher.bench(|| black_box(&entries).iter().filter_map(|(key, _)| black_box(&map).get(key)).sum::<u64>());
}

#[divan::bench(args = SIZES)]
fn btree_map_iter(bencher: Bencher<'_, '_>, n: usize) {
    let map = entries(n).into_iter().collect::<BTreeMap<Key, u64>>();
    bencher.bench(|| black_box(&map).iter().count());
}

#[divan::bench(args = SIZES)]
fn compact_map_build(bencher: Bencher<'_, '_>, n: usize) {
    let entries = entries(n);
    bencher.bench(|| black_box(&entries).iter().copied().collect::<CompactMap<Key, u64, SMALL>>());
}

#[divan::bench(args = SIZES)]
fn compact_map_clone(bencher: Bencher<'_, '_>, n: usize) {
    let map = entries(n).into_iter().collect::<CompactMap<Key, u64, SMALL>>();
    bencher.bench(|| black_box(&map).clone());
}

#[divan::bench(args = SIZES)]
fn compact_map_get(bencher: Bencher<'_, '_>, n: usize) {
    let entries = entries(n);
    let map = entries.iter().copied().collect::<CompactMap<Key, u64, SMALL>>();
    bencher.bench(|| black_box(&entries).iter().filter_map(|(key, _)| black_box(&map).get(key)).sum::<u64>());
}

#[divan::bench(args = SIZES)]
fn compact_map_iter(bencher: Bencher<'_, '_>, n: usize) {
    let map = entries(n).into_iter().collect::<CompactMap<Key, u64, SMALL>>();
    bencher.bench(|| black_box(&map).iter().count());
}
