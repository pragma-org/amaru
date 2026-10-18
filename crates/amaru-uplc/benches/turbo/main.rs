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

#![expect(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Instant,
};

use amaru_kernel::{
    Epoch, PROTOCOL_VERSION_8, PROTOCOL_VERSION_9, PROTOCOL_VERSION_10, PROTOCOL_VERSION_11, PlutusVersion,
    ProtocolVersion, utils::duration,
};
use amaru_minicbor_extra::decode_bytes;
use amaru_uplc::{
    arena::Arena,
    arena_pool::ArenaPool,
    binder::DeBruijn,
    flat,
    machine::{CostModel, ExBudget},
};
use itertools::Itertools;
use rayon::prelude::*;
use serde_json::json;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

const TURBO_REPORT: &str = "benches/turbo/uplc-turbo.json";
const TURBO_DATA_DIR: &str = "benches/turbo/samples";
const TURBO_ARCHIVE_URL: &str = "https://pub-2239d82d9a074482b2eb2c886191cb4e.r2.dev/turbo.tar.zst";

// Initial capacity of the arena, in bytes.
const BUMP_ARENA_CAPACITY: usize = 20 * 1048576; // 20 MB

// Total number of samples per script.
const SAMPLE_COUNT: usize = 50;

// Benchmarks run in parallel and there's often noise, so we keep the best 30 out of 50 runs to keep
// results somewhat uniform across re-runs. The point is to identify outliers and regressions.
const RETAINED_SAMPLE_COUNT: usize = 30;

// All known samples, loaded lazily
static SAMPLES: LazyLock<Vec<(Epoch, PathBuf)>> = LazyLock::new(samples);

fn samples() -> Vec<(Epoch, PathBuf)> {
    let data_dir = Path::new(TURBO_DATA_DIR);
    let filters = std::env::var("UPLC_TURBO_FILTER").ok().map(|value| {
        value.split(',').map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned).collect::<Vec<_>>()
    });

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

    let limit = std::env::var("UPLC_TURBO_LIMIT")
        .map(|value| value.parse().expect("invalid UPLC_TURBO_LIMIT"))
        .unwrap_or(usize::MAX);

    // Collect all files from all subdirectories.
    entries
        .into_iter()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|dir| match dir.file_stem().unwrap_or_default().to_string_lossy().parse::<u64>() {
            Ok(epoch) => fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .map(|entry| (Epoch::from(epoch), entry.unwrap().path()))
                .collect::<Vec<_>>(),
            Err(_) => vec![],
        })
        .filter(|(_epoch, path)| {
            filters.as_ref().is_none_or(|filters| {
                let stem = path.file_stem().and_then(|stem| stem.to_str());
                let name = path.file_name().and_then(|name| name.to_str());
                filters.iter().any(|filter| Some(filter.as_str()) == stem || Some(filter.as_str()) == name)
            })
        })
        .sorted()
        .take(limit)
        .collect()
}

fn collect_scripts(files: &[(Epoch, PathBuf)]) -> Vec<(String, Vec<u8>, PlutusVersion, ProtocolVersion)> {
    files
        .iter()
        .map(|(epoch, path)| {
            let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();

            let plutus_version = detect_plutus_version(file_name);

            let protocol_version = infer_protocol_version(*epoch);

            let cbor = std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

            let flat = minicbor::decode::<CborWrapped>(&cbor)
                .unwrap_or_else(|e| panic!("Failed to decode CBOR from {}: {}", path.display(), e))
                .0;

            (file_name.to_string(), flat, plutus_version, protocol_version)
        })
        .collect::<Vec<_>>()
}

#[derive(Debug, Copy, Clone)]
struct Measurement {
    cpu_us: u64,
    mem_bytes: u64,
    budget: ExBudget,
}

impl Measurement {
    fn new(
        arena: &Arena,
        flat: &[u8],
        plutus_version: PlutusVersion,
        protocol_version: ProtocolVersion,
        costs: CostModel,
    ) -> Self {
        let (program, _) =
            flat::decode::<DeBruijn>(arena, flat, plutus_version, protocol_version).expect("Failed to decode");

        let mut meter = Instant::now();

        let arena_before_eval = arena.used_bytes();
        let result = program.eval(arena, costs, ExBudget::max());

        let budget = result.info.consumed_budget;
        let _term = result.term.expect("failed to evaluate");

        let mem_bytes = arena.used_bytes().saturating_sub(arena_before_eval) as u64;
        let cpu_us = duration::elapsed_and_reset(&mut meter);

        Self { cpu_us, mem_bytes, budget }
    }
}

fn main() {
    eprintln!("Collecting samples...");
    let scripts = collect_scripts(&SAMPLES);

    eprintln!("Benchmarking...");
    let arenas = ArenaPool::new(8, BUMP_ARENA_CAPACITY);
    let benchmarks = scripts
        .iter()
        .enumerate()
        .map(|(ix, (id, flat_bytes, plutus_version, protocol_version))| {
            eprintln!("[{ix}/{} {id}", scripts.len());

            let costs = cost_model(*plutus_version, *protocol_version);

            let measurements: Vec<Measurement> = (0..SAMPLE_COUNT)
                .into_par_iter()
                .map(|_| {
                    Measurement::new(&arenas.acquire(), flat_bytes, *plutus_version, *protocol_version, costs.clone())
                })
                .collect();

            json!({
                "bench": id,
                "plutus_version": plutus_version.to_string(),
                "protocol_version": protocol_version.to_string(),
                "budget_cpu": measurements[0].budget.cpu,
                "budget_mem": measurements[0].budget.mem,
                "cpu_us": mean(measurements.iter().map(|m| m.cpu_us).collect()),
                "mem_bytes": mean(measurements.iter().map(|m| m.mem_bytes).collect()),
            })
        })
        .collect::<Vec<_>>();

    let out = PathBuf::from(std::env::var("UPLC_TURBO_REPORT").unwrap_or_else(|_| TURBO_REPORT.to_string()));

    eprintln!("Writing report to {out:?}...");

    let report = json!({
        "version": "1",
        "scenario": "uplc-turbo",
        "generated_at": duration::since_unix_epoch(),
        "implementation": { "name": "amaru" },
        "machine": machine(),
        "benchmarks": benchmarks,
    });

    fs::write(&out, serde_json::to_vec_pretty(&report).expect("serialize UPLC turbo report"))
        .unwrap_or_else(|error| panic!("failed to write {out:?}: {error}"));
}

// -------------------------------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
struct CborWrapped(Vec<u8>);

impl<'d, C> minicbor::Decode<'d, C> for CborWrapped {
    fn decode(d: &mut minicbor::Decoder<'d>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let bytes = decode_bytes(d)?;
        Ok(CborWrapped(bytes.into_owned()))
    }
}

fn cost_model(plutus_version: PlutusVersion, protocol_version: ProtocolVersion) -> CostModel {
    CostModel::default_for(plutus_version, protocol_version)
}

fn machine() -> serde_json::Value {
    let system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything().without_frequency())
            .with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    let cpu = system.cpus().first().map(|cpu| cpu.brand().trim()).filter(|brand| !brand.is_empty());

    json!({
        "os": System::name().unwrap_or_else(|| std::env::consts::OS.to_owned()),
        "os_version": System::long_os_version(),
        "architecture": std::env::consts::ARCH,
        "cpu": cpu,
        "logical_cores": system.cpus().len().max(1),
        "memory_bytes": system.total_memory(),
    })
}

fn detect_plutus_version(file_name: &str) -> PlutusVersion {
    if file_name.ends_with("v1") || file_name.ends_with("v1.flat") {
        PlutusVersion::V1
    } else if file_name.ends_with("v2") || file_name.ends_with("v2.flat") {
        PlutusVersion::V2
    } else if file_name.ends_with("v3") || file_name.ends_with("v3.flat") {
        PlutusVersion::V3
    } else {
        panic!("cannot decode plutus version from filename: {file_name}");
    }
}

// See also: https://cardano.org/hardforks/
fn infer_protocol_version(epoch: Epoch) -> ProtocolVersion {
    if epoch >= Epoch::from(644) {
        PROTOCOL_VERSION_11
    } else if epoch >= Epoch::from(537) {
        PROTOCOL_VERSION_10
    } else if epoch >= Epoch::from(507) {
        PROTOCOL_VERSION_9
    } else {
        PROTOCOL_VERSION_8
    }
}

fn mean(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    let retained = samples.iter().take(RETAINED_SAMPLE_COUNT);
    let count = retained.clone().count().max(1);
    (retained.map(|sample| *sample as f64).sum::<f64>() / count as f64).ceil() as u64
}
