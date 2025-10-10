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
use amaru_stores::rocksdb::{
    RocksDB, RocksDBHistoricalStores, RocksDbConfig, consensus::RocksDBStore,
};
use anyhow::anyhow;
use clap::Parser;
use std::{
    fs::{self, File},
    io::Read,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
use tracing::info;

use amaru_kernel::{
    EraHistory, HeaderBody, Point, PseudoHeader, RawBlock, default_chain_dir, default_ledger_dir,
    network::NetworkName,
    protocol_parameters::{ConsensusParameters, GlobalParameters},
};
use amaru_ledger::{block_validator::BlockValidator, rules::parse_block};
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

/// Load archives containing blocks and sort them based on file name (filesystem access doesn't guarantee any ordering)
fn list_archive_names(network: NetworkName) -> Result<Vec<String>, Box<dyn std::error::Error>> {
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
    Ok(archives)
}

fn load_archive(
    network: NetworkName,
    archive_path: &String,
) -> Result<Archive<GzDecoder<File>>, Box<dyn std::error::Error>> {
    let file = fs::File::open(format!("data/{}/blocks/{}", network, archive_path))?;
    let gz = GzDecoder::new(file);
    Ok(Archive::new(gz))
}

fn create_praos_chain_store(
    global_parameters: GlobalParameters,
    chain_store: Arc<dyn ChainStore<PseudoHeader<HeaderBody>>>,
    era_history: &EraHistory,
) -> PraosChainStore<PseudoHeader<HeaderBody>> {
    let consensus_parameters = Arc::new(ConsensusParameters::new(
        global_parameters,
        era_history,
        Default::default(),
    ));
    PraosChainStore::new(consensus_parameters, chain_store)
}

async fn load_blocks(
    archive: &mut Archive<GzDecoder<File>>,
) -> Result<Vec<(Point, RawBlock)>, Box<dyn std::error::Error>> {
    let archive_entries = archive.entries()?;
    let mut entries_with_keys: Vec<(_, _)> = Vec::with_capacity(archive_entries.size_hint().0);

    for entry in archive_entries {
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
    Ok(entries_with_keys)
}

/// Process blocks as if they were processed by the full node
/// Particularly all on disk side-effects are performed
async fn process_block(
    chain_store: &Arc<dyn ChainStore<PseudoHeader<HeaderBody>>>,
    praos_chain_store: &PraosChainStore<PseudoHeader<HeaderBody>>,
    block_validator: &BlockValidator<RocksDB, RocksDBHistoricalStores>,
    point: &Point,
    raw_block: &RawBlock,
) -> Result<(), Box<dyn std::error::Error>> {
    let block = parse_block(raw_block)?;
    let header = block.header.unwrap().into();
    chain_store.store_header(&header)?;
    chain_store.store_block(&point.hash(), raw_block)?;
    praos_chain_store.evolve_nonce(&header)?;
    let _ = block_validator
        .roll_forward_block(point, raw_block)
        .await
        .map_err(|err| anyhow!("Error processing block at point {:?}: {:?}", point, err))?;

    Ok(())
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

    let era_history: &EraHistory = network.into();
    let global_parameters: &GlobalParameters = network.into();
    let block_validator = new_block_validator(network, ledger_dir)?;
    let tip = block_validator.get_tip();
    let chain_store: Arc<dyn ChainStore<PseudoHeader<HeaderBody>>> =
        Arc::new(RocksDBStore::new(&RocksDbConfig::new(chain_dir))?);
    let praos_chain_store =
        create_praos_chain_store(global_parameters.clone(), chain_store.clone(), era_history);

    // Collect .tar.gz files
    let archive_names = list_archive_names(network)?;

    let mut processed = 0;
    // Process relevant points
    let before = Instant::now();

    for archive_name in &archive_names {
        let mut archive = load_archive(network, archive_name)?;
        let blocks = load_blocks(&mut archive).await?;

        for (point, raw_block) in blocks
            .iter()
            // Do not process points already in the ledger
            .filter(|(point, _)| point.slot_or_default() > tip.slot_or_default())
        {
            process_block(
                &chain_store,
                &praos_chain_store,
                &block_validator,
                point,
                raw_block,
            )
            .await?;

            processed += 1;
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
