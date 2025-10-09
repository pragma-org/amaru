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

use amaru_consensus::consensus::store::PraosChainStore;
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;
use std::{fs, io::Read, path::PathBuf, sync::Arc, time::Instant};
use tracing::info;

use amaru_kernel::{
    EraHistory, HeaderBody, Point, PseudoHeader, RawBlock, default_chain_dir, default_ledger_dir,
    network::NetworkName,
    protocol_parameters::{ConsensusParameters, GlobalParameters},
};
use amaru_ledger::{rules::parse_block, store::HistoricalStores};
use amaru_ouroboros_traits::{ChainStore, Praos, can_validate_blocks::CanValidateBlocks};

use flate2::read::GzDecoder;
use tar::Archive;

use crate::cmd::new_block_validator;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
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
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR")]
    chain_dir: Option<PathBuf>,

    /// Ingest blocks until (and including) the given slot.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "INGEST_UNTIL_SLOT")]
    ingest_until_slot: Option<u64>,

    /// Ingest at most the given number of blocks.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "INGEST_MAXIMUM_BLOCKS")]
    ingest_maximum_blocks: Option<usize>,
}

#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(network).into());

    let global_parameters: &GlobalParameters = network.into();
    let block_validator = new_block_validator(network, ledger_dir)?;
    let chain_store: Arc<dyn ChainStore<PseudoHeader<HeaderBody>>> =
        Arc::new(RocksDBStore::new(&RocksDbConfig::new(chain_dir))?);

    // Collect .tar.gz files
    let mut archives: Vec<_> = fs::read_dir(format!("data/{}/blocks", network))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()? == "gz" {
                Some(
                    path.file_name()?
                        .to_os_string()
                        .to_string_lossy()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect();

    archives.sort_by(|a, b| {
        let a = a.split_once(".").unwrap_or_default().0;
        let b = b.split_once(".").unwrap_or_default().0;
        a.parse::<u32>()
            .unwrap_or_default()
            .cmp(&b.parse::<u32>().unwrap_or_default())
    });

    let tip = block_validator.get_tip();

    let mut processed = 0;
    // Process relevant points
    let before = Instant::now();
    for archive_path in &archives {
        let file = fs::File::open(format!("data/{}/blocks/{}", network, archive_path))?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        let mut entries_with_keys: Vec<(_, _)> =
            Vec::with_capacity(archive.entries()?.size_hint().0);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;

            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                //let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                let (slot_str, hash_str) = file_name
                    .strip_suffix(".cbor")
                    .unwrap_or(file_name)
                    .split_once('.')
                    .unwrap_or(("0", ""));
                let point = Point::Specific(
                    slot_str.parse().unwrap_or_default(),
                    hex::decode(hash_str).unwrap_or_default(),
                );
                let mut block_data = Vec::new();
                entry.read_to_end(&mut block_data)?;
                entries_with_keys.push((point, RawBlock::from(&*block_data)));
            }
        }

        // Sort by numeric key
        entries_with_keys.sort_by_key(|(num, _)| num.clone());

        for (point, raw_block) in entries_with_keys.iter_mut() {
            // Do not process points already in the ledger
            if point.slot_or_default() <= tip.slot_or_default() {
                continue;
            }
            processed += 1;

            let block = parse_block(raw_block)?;
            let header = block.header.unwrap().into();
            chain_store.store_header(&header)?;
            chain_store.store_block(&point.hash(), raw_block)?;
            let era_history: &EraHistory = network.into();
            let consensus_parameters = Arc::new(ConsensusParameters::new(
                global_parameters.clone(),
                era_history,
                Default::default(),
            ));
            PraosChainStore::new(consensus_parameters, chain_store.clone())
                .evolve_nonce(&header)?;

            if let Err(err) = block_validator
                .roll_forward_block(point, raw_block)
                .await
                .unwrap()
            {
                panic!("Error processing block at point {:?}: {:?}", point, err);
            }
        }
    }

    let duration = Instant::now().saturating_duration_since(before);
    info!(
        "Processed {} blocks in {} seconds ({} blocks/s)",
        processed,
        duration.as_secs(),
        processed / duration.as_secs()
    );

    Ok(())
}
