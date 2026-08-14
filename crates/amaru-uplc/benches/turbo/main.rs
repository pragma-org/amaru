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

use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, Instant},
};

use amaru_kernel::{PROTOCOL_VERSION_10, PlutusVersion};
use amaru_minicbor_extra::decode_bytes;
use amaru_uplc::{arena::Arena, binder::DeBruijn, flat, machine::ExBudget};
use bumpalo::Bump;
use divan::Bencher;
use itertools::Itertools;
use rayon::prelude::*;

#[cfg(feature = "alloc_profiler")]
#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

const TURBO_DATA_DIR: &str = "benches/turbo/samples";
const TURBO_ARCHIVE_URL: &str = "https://pub-2239d82d9a074482b2eb2c886191cb4e.r2.dev/turbo.tar.xz";

// Initial capacity of the arena, in bytes.
const BUMP_ARENA_CAPACITY: usize = 1048576; // 1 MB

// All known samples, loaded lazily
static SAMPLES: LazyLock<Vec<PathBuf>> = LazyLock::new(samples);

#[derive(Debug)]
struct CborWrapped(Vec<u8>);

impl<'d, C> minicbor::Decode<'d, C> for CborWrapped {
    fn decode(d: &mut minicbor::Decoder<'d>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
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

fn samples() -> Vec<PathBuf> {
    let data_dir = Path::new(TURBO_DATA_DIR);

    // Check directory exists and has content
    let entries: Vec<_> =
        fs::read_dir(data_dir).unwrap_or_else(|_| panic!("Failed to read directory: {}", data_dir.display())).collect();

    if entries.len() <= 1 {
        panic!(
            "missing turbo benchmarks; download archive at {} and unpack it under {}",
            TURBO_ARCHIVE_URL,
            data_dir.to_str().unwrap_or_default(),
        );
    }

    // Collect all files from all subdirectories.
    entries
        .into_iter()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|dir| fs::read_dir(&dir).into_iter().flatten().map(|entry| entry.unwrap().path()).collect::<Vec<_>>())
        .sorted()
        .collect()
}

fn collect_scripts(files: &[PathBuf]) -> Vec<(String, Vec<u8>, PlutusVersion)> {
    files
        .iter()
        .map(|path| {
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();

            let plutus_version = detect_plutus_version(file_stem);

            let cbor = std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

            let flat = minicbor::decode::<CborWrapped>(&cbor)
                .unwrap_or_else(|e| panic!("Failed to decode CBOR from {}: {}", path.display(), e))
                .0;

            (file_stem.to_string(), flat, plutus_version)
        })
        .collect::<Vec<_>>()
}

fn bench_turbo(arena: &mut Arena) -> impl FnMut(Vec<u8>, PlutusVersion) + use<'_> {
    move |flat, plutus_version| {
        // TODO: We are hardcoding 10, the current mainnet protocol version. This should be an argument
        let (program, _) = flat::decode::<DeBruijn>(arena, &flat, PROTOCOL_VERSION_10).expect("Failed to decode");

        let result = program.eval_version_budget(arena, plutus_version, ExBudget::max());

        let _term = result.term.expect("Failed to evaluate");

        arena.reset();
    }
}

fn analyze_turbo(arena: &mut Arena, flat: Vec<u8>, plutus_version: PlutusVersion) -> (Duration, Duration, Duration) {
    let instant = Instant::now();

    let (program, _) = flat::decode::<DeBruijn>(arena, &flat, PROTOCOL_VERSION_10).expect("Failed to decode");
    let elapsed_unflat = instant.elapsed();

    let result = program.eval_version_budget(arena, plutus_version, ExBudget::max());
    let elapsed_eval = instant.elapsed();

    let _term = result.term.expect("Failed to evaluate");

    arena.reset();
    let elapsed_reset = instant.elapsed();

    (
        elapsed_unflat,
        elapsed_eval.saturating_sub(elapsed_unflat),
        elapsed_reset.saturating_sub(elapsed_unflat).saturating_sub(elapsed_eval),
    )
}

#[divan::bench(sample_count = SAMPLES.len() as u32)]
fn turbo(bencher: Bencher) {
    let mut arena = Arena::from_bump(Bump::with_capacity(BUMP_ARENA_CAPACITY));
    let mut scripts = collect_scripts(&SAMPLES);
    let mut f = bench_turbo(&mut arena);
    bencher
        .with_inputs(|| scripts.pop().unwrap())
        .bench_local_values(|(_, flat, plutus_version)| f(flat, plutus_version));
}

fn analyze_slow_evals(threshold: Duration) {
    eprintln!("Collecting script executions slower than {}ms", threshold.as_millis());
    let scripts = collect_scripts(&SAMPLES);
    scripts
        .into_par_iter()
        .map_init(
            || Arena::from_bump(Bump::with_capacity(BUMP_ARENA_CAPACITY)),
            |arena, (filename, flat, plutus_version)| {
                let (elapsed_unflat, elapsed_eval, elapsed_reset) = analyze_turbo(arena, flat, plutus_version);
                if elapsed_unflat + elapsed_eval + elapsed_reset > threshold {
                    Some((filename, elapsed_unflat, elapsed_eval, elapsed_reset))
                } else {
                    None
                }
            },
        )
        .filter_map(|result| result)
        .for_each(|(filename, elapsed_unflat, elapsed_eval, elapsed_reset)| {
            fn display_duration(d: Duration) -> String {
                if d > Duration::from_millis(10) {
                    format!("{}ms", d.as_millis())
                } else if d > Duration::from_nanos(10000) {
                    format!("{}μs", d.as_nanos() / 1000)
                } else {
                    format!("{}ns", d.as_nanos())
                }
            }

            println!(
                "{filename} in {}ms\n\telapsed unflat: {}\n\telapsed eval:   {}\n\telapsed reset:  {}",
                (elapsed_unflat + elapsed_eval + elapsed_reset).as_millis(),
                display_duration(elapsed_unflat),
                display_duration(elapsed_eval),
                display_duration(elapsed_reset),
            );
        })
}

fn main() {
    match std::env::var("UPLC_TURBO_ANALYZE_SLOW") {
        Ok(threshold) => analyze_slow_evals(Duration::from_millis(threshold.parse().expect("invalid time limit"))),
        Err(..) => divan::main(),
    }
}
