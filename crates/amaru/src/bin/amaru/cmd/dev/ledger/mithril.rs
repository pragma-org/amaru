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

use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

use amaru::{
    default_data_dir, default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{NetworkName, Point, cbor};
use amaru_ledger::store::ReadStore;
use amaru_mithril::{
    BLOCKS_PER_ARCHIVE, archive_name_for_blocks, download_from_mithril, from_chunk_for_resume_point, get_latest_chunk,
    latest_archive, list_existing_archives, package_blocks, parse_header_slot_and_hash, read_stable_blocks_after_point,
    resume_point_for_archives,
};
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig};
use clap::Parser;
use tracing::{info, warn};

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or `testnet:<magic>` where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK_NAME",
        env = "AMARU_NETWORK",
        default_value_t = NetworkName::Preprod,
        verbatim_doc_comment
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_LEDGER_DIR", verbatim_doc_comment)]
    ledger_dir: Option<PathBuf>,

    /// Path of the mithril snapshots on-disk storage.
    #[arg(
        long,
        value_name = "DIR",
        default_value = "mithril-snapshots",
        env = "AMARU_MITHRIL_SNAPSHOTS_DIR",
        verbatim_doc_comment
    )]
    snapshots_dir: PathBuf,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let target_dir = args.snapshots_dir.join(network.to_string());
    fs::create_dir_all(&target_dir)?;

    let immutable_dir = target_dir.join("immutable");
    let blocks_dir = PathBuf::from(format!("{}/blocks", default_data_dir(network)));

    let store = RocksDB::new(&RocksDbConfig::new(ledger_dir))?;
    let tip = store.tip()?;
    let mut existing_archives = list_existing_archives(&blocks_dir)?;
    let tail_archive = latest_archive(&existing_archives);

    // Determine the chunk to start from
    let mut resume_point = resume_point_for_archives(&existing_archives);
    if resume_point == Point::Origin {
        // Origin means fewer than two archives exist, so there is no complete archive to
        // resume from. Resuming from Origin would download and package the whole chain
        // since genesis, but the ledger already covers everything up to its tip (e.g.
        // after a snapshot bootstrap), so only blocks after the tip are needed. When a
        // single tail archive exists it starts right after the tip, and the packaging
        // loop below retains or replaces it. This relies on the chunk containing the tip
        // being available locally, which from_chunk_for_resume_point ensures by starting
        // the download one chunk before the tip.
        resume_point = tip;
    }

    let latest_chunk = get_latest_chunk(&immutable_dir)?;
    let from_chunk = from_chunk_for_resume_point(network, latest_chunk, resume_point)?;

    info!(
        tip=%tip, resume_point=%resume_point, from_chunk=%from_chunk,
        "Downloading mithril immutable chunks"
    );

    download_from_mithril(
        network,
        target_dir,
        from_chunk,
        Arc::new(|size: usize, template: &str| {
            Box::new(TerminalProgressBar::new(size as u64, template)) as Box<dyn ProgressBar + Send + Sync>
        }),
    )
    .await?;

    info!("Packaging blocks into .tar.gz files");

    // Read blocks from the immutable storage and package them into .tar.gz files.
    let mut iter = read_stable_blocks_after_point(&immutable_dir, network, resume_point)?;
    loop {
        let chunk: Vec<_> = iter.by_ref().take(BLOCKS_PER_ARCHIVE).collect::<Result<_, _>>()?;
        if chunk.is_empty() {
            break;
        }
        let map: BTreeMap<Point, &Vec<u8>> = chunk
            .iter()
            .map(|cbor| {
                let parsed = parse_header_slot_and_hash(cbor)?;
                let point = Point::Specific(parsed.slot.into(), parsed.header_hash.into());
                Ok((point, cbor))
            })
            .collect::<Result<BTreeMap<Point, &Vec<u8>>, cbor::decode::Error>>()?;

        #[allow(clippy::expect_used)]
        let archive_name = archive_name_for_blocks(&map).expect("chunk map is non-empty here by construction");
        if let Some(tail_archive) = &tail_archive {
            let first_block = map.first_key_value().map(|(point, _)| point);

            if first_block == Some(&tail_archive.first_block) && archive_name == tail_archive.file_name {
                info!(archive = %archive_name, "Retaining existing tail archive");
                continue;
            }

            if first_block == Some(&tail_archive.first_block) && archive_name != tail_archive.file_name {
                let stale_archive_path = blocks_dir.join(&tail_archive.file_name);
                fs::remove_file(&stale_archive_path)?;
                existing_archives.remove(&tail_archive.file_name);
                info!(archive = %tail_archive.file_name, replacement = %archive_name, "Replacing tail archive");
            }
        }
        if existing_archives.contains(&archive_name) {
            debug_assert!(false, "encountered an already archived non-tail batch: {}", archive_name);
            warn!(archive = %archive_name, "Encountered an already archived non-tail batch");
            continue;
        }

        let dir = package_blocks(&blocks_dir, &map)?;
        existing_archives.insert(archive_name);

        info!(blocks = map.len(), dir, "Created archive batch");
    }

    info!("Done");

    Ok(())
}
