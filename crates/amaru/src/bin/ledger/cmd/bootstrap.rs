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

use clap::Parser;
use std::{collections::BTreeSet, fs, io::Read, path::PathBuf};
use tracing::info;

use amaru_kernel::{
    EraHistory, Point, RawBlock, default_ledger_dir, network::NetworkName,
    protocol_parameters::GlobalParameters,
};
use amaru_ledger::block_validator::BlockValidator;
use amaru_ouroboros_traits::can_validate_blocks::CanValidateBlocks;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

use flate2::read::GzDecoder;
use tar::Archive;


#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Ingest blocks until (and including) the given slot.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "INGEST_UNTIL_SLOT")]
    ingest_until_slot: Option<u64>,

    /// Ingest at most the given number of blocks.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "INGEST_MAXIMUM_BLOCKS")]
    ingest_maximum_blocks: Option<usize>,

    #[clap(long, action, env("AMARU_WITH_OPEN_TELEMETRY"))]
    with_open_telemetry: bool,

    #[clap(long, action, env("AMARU_WITH_JSON_TRACES"))]
    with_json_traces: bool,

    #[arg(long, value_name = "STRING", env("AMARU_SERVICE_NAME"), default_value_t = DEFAULT_SERVICE_NAME.to_string())]
    service_name: String,

    #[arg(long, value_name = "URL", env("AMARU_OTLP_SPAN_URL"), default_value_t = DEFAULT_OTLP_SPAN_URL.to_string())]
    otlp_span_url: String,

    #[arg(long, value_name = "URL", env("AMARU_OTLP_METRIC_URL"), default_value_t = DEFAULT_OTLP_METRIC_URL.to_string())]
    otlp_metric_url: String,
}

const DEFAULT_SERVICE_NAME: &str = "amaru-ledger";

const DEFAULT_OTLP_SPAN_URL: &str = "http://localhost:4317";

const DEFAULT_OTLP_METRIC_URL: &str = "http://localhost:4318/v1/metrics";

pub fn filter_points(
    points: &[Point],
    low: u64,
    high: Option<u64>,
    max_count: Option<usize>,
) -> Vec<Point> {
    let mut sorted = points.to_vec();
    sorted.sort_by_key(|p| p.slot_or_default());

    let start = sorted
        .binary_search_by_key(&(low + 1), |p| p.slot_or_default().into())
        .unwrap_or_else(|pos| pos);

    let mut end = if let Some(high) = high {
        sorted
            .binary_search_by_key(&high, |p| p.slot_or_default().into())
            .map(|pos| pos + 1)
            .unwrap_or_else(|pos| pos)
    } else {
        sorted.len()
    };

    if let Some(max) = max_count {
        end = end.min(start.saturating_add(max));
    }

    sorted[start..end].to_vec()
}

pub fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());

    let era_history: &EraHistory = network.into();
    let global_parameters: &GlobalParameters = network.into();
    let config = RocksDbConfig::new(ledger_dir);
    let store = RocksDBHistoricalStores::new(&config, u64::MAX);
    let ledger = BlockValidator::new(
        RocksDB::new(&config)?,
        store,
        network,
        era_history.clone(),
        global_parameters.clone(),
    )?;

    // Collect .tar.gz files
    let mut archives: Vec<_> = fs::read_dir(format!("data/{}/blocks", network))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()? == "gz" {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    archives.sort();

    let tip = ledger.get_tip();

    // Collect all points
    let mut all_points = Vec::new();
    for archive_path in &archives {
        let file = fs::File::open(archive_path)?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?;
            if path.extension().map(|ext| ext == "cbor").unwrap_or(false) {
                let file_name = path.file_name().unwrap().to_string_lossy();
                let (slot_str, hash_str) = file_name.split_once('.').unwrap_or(("0", ""));
                let point = Point::Specific(
                    slot_str.parse().unwrap_or_default(),
                    hex::decode(hash_str).unwrap_or_default(),
                );
                if point.slot_or_default() > tip.slot_or_default() {
                    all_points.push(point);
                }
            }
        }
    }

    // Filter according to CLI args
    let subset = filter_points(
        &all_points,
        tip.slot_or_default().into(),
        args.ingest_until_slot,
        args.ingest_maximum_blocks,
    );

    let subset_set: BTreeSet<_> = subset.iter().cloned().collect();

    // Process relevant points
    for archive_path in &archives {
        let file = fs::File::open(archive_path)?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;
            if path.extension().map(|ext| ext == "cbor").unwrap_or(false) {
                let file_name = path.file_name().unwrap().to_string_lossy();
                let (slot_str, hash_str) = file_name.split_once('.').unwrap_or(("0", ""));
                let point = Point::Specific(
                    slot_str.parse().unwrap_or_default(),
                    hex::decode(hash_str).unwrap_or_default(),
                );

                if !subset_set.contains(&point) {
                    continue;
                }

                let mut block_data = Vec::new();
                entry.read_to_end(&mut block_data)?;

                if let Err(err) = ledger
                    .roll_forward_block(&point, &RawBlock::from(&*block_data))
                    .unwrap()
                {
                    panic!("Error processing block at point {:?}: {:?}", point, err);
                }
            }
        }
    }

    info!(
        "Processed {} blocks from slot {} to slot {}",
        subset.len(),
        subset
            .first()
            .map(|p| p.slot_or_default())
            .unwrap_or_default(),
        subset
            .last()
            .map(|p| p.slot_or_default())
            .unwrap_or_default()
    );

    Ok(())
}
