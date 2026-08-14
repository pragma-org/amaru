// Copyright 2025 PRAGMA
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

use amaru_minicbor_extra::decode_bytes;
use amaru_uplc::{arena::Arena, binder::DeBruijn, flat, machine::PlutusVersion};
use bumpalo::Bump;
use criterion::{criterion_group, Criterion};
use itertools::Itertools;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const TURBO_DATA_DIR: &str = "benches/benchmarks/turbo";
const TURBO_ARCHIVE_URL: &str = "https://pub-2239d82d9a074482b2eb2c886191cb4e.r2.dev/turbo.tar.xz";

// Initial capacity of the arena, in bytes.
const BUMP_ARENA_CAPACITY: usize = 524288;

// How many groups to shard the benchmarks into.
const DEFAULT_NUM_GROUPS: usize = 8;

#[derive(Debug)]
struct CborWrapped(Vec<u8>);

impl<'d, C> minicbor::Decode<'d, C> for CborWrapped {
    fn decode(
        d: &mut minicbor::Decoder<'d>,
        _ctx: &mut C,
    ) -> Result<Self, minicbor::decode::Error> {
        let bytes = decode_bytes(d)?;
        Ok(CborWrapped(bytes.into_owned()))
    }
}

fn detect_plutus_version(file_name: &str) -> PlutusVersion {
    if file_name.ends_with("v1") {
        PlutusVersion::V1
    } else if file_name.ends_with("v2") {
        PlutusVersion::V2
    } else if file_name.ends_with("v3") {
        PlutusVersion::V3
    } else {
        panic!("cannot decode plutus version from filename: {file_name}");
    }
}

fn bench_turbo(c: &mut Criterion) {
    let num_groups = std::env::var("UPLC_TURBO_NUM_GROUPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_NUM_GROUPS);

    assert!(
        num_groups > 0,
        "UPLC_TURBO_NUM_GROUPS cannot be smaller than 1"
    );

    let data_dir = Path::new(TURBO_DATA_DIR);

    // Check directory exists and has content
    let entries: Vec<_> = fs::read_dir(data_dir)
        .unwrap_or_else(|_| panic!("Failed to read directory: {}", data_dir.display()))
        .collect();

    if entries.len() <= 1 {
        panic!(
            "missing turbo benchmarks; download archive at {} and unpack it under {}",
            TURBO_ARCHIVE_URL,
            data_dir.to_str().unwrap_or_default(),
        );
    }

    // Collect all files from all subdirectories
    let all_files: Vec<_> = entries
        .into_iter()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|subdir| {
            fs::read_dir(&subdir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
        })
        .sorted()
        .collect();

    // Group bytes by index for even distribution
    let groups: BTreeMap<usize, Vec<&Path>> = all_files.iter().enumerate().fold(
        BTreeMap::new(),
        |mut acc, (ix, file): (usize, &PathBuf)| {
            acc.entry(ix % num_groups)
                .and_modify(|files: &mut Vec<_>| files.push(file.as_path()))
                .or_insert(vec![file.as_path()]);
            acc
        },
    );

    // Run benchmarks in separate indexed groups, to allow easily filtering them. The size of the
    // groups can be controlled using UPLC_TURBO_NUM_GROUPS; while one can then select subgroups
    // using benchmark filters:
    //
    // cargo bench turbo::0/
    // cargo bench turbo::1/
    // cargo bench turbo::2/
    for (ix, files) in groups {
        let mut group = c.benchmark_group(format!("turbo::{}", ix));

        for path in files {
            let file_stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();

            let folder_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or_default();

            let file_name = format!("{}/{}", folder_name, file_stem);

            let plutus_version = detect_plutus_version(&file_name);

            let cbor = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

            let flat = minicbor::decode::<CborWrapped>(&cbor)
                .unwrap_or_else(|e| panic!("Failed to decode CBOR from {}: {}", path.display(), e))
                .0;

            let mut arena = Arena::from_bump(Bump::with_capacity(BUMP_ARENA_CAPACITY));

            group.bench_function(&file_name, |b| {
                b.iter(|| {
                    let program = flat::decode::<DeBruijn>(&arena, &flat, plutus_version, 10)
                        .expect("Failed to decode");

                    let result = program.eval_version(&arena, plutus_version);

                    let _term = result.term.expect("Failed to evaluate");

                    arena.reset();
                })
            });
        }

        group.finish();
    }
}

criterion_group! {
    name = turbo;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(10))
        .measurement_time(Duration::from_millis(100));
    targets = bench_turbo
}
