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

use crate::cmd::new_block_validator;
use amaru::{DEFAULT_NETWORK, default_chain_dir, default_data_dir, default_ledger_dir};
use amaru_consensus::store::PraosChainStore;
use amaru_kernel::cardano::network_block::NetworkBlock;
use amaru_kernel::{
    BlockHeader, ConsensusParameters, EraHistory, GlobalParameters, Hash, NetworkName, Point,
    RawBlock, to_cbor,
};
use amaru_ledger::block_validator::BlockValidator;
use amaru_ouroboros::praos::header;
use amaru_ouroboros::{ChainStore, Praos, can_validate_blocks::CanValidateBlocks};
use amaru_stores::rocksdb::{
    RocksDB, RocksDBHistoricalStores, RocksDbConfig, consensus::RocksDBStore,
};
use anyhow::anyhow;
use flate2::read::GzDecoder;
use rayon::prelude::*;
use std::{
    fs::{self, File},
    io::Read,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Instant,
};
use tar::Archive;
use tracing::info;

#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = DEFAULT_NETWORK,
        verbatim_doc_comment
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_LEDGER_DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_CHAIN_DIR")]
    chain_dir: Option<PathBuf>,

    /// Ingest blocks until (and including) the given slot.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "SLOT", env = "AMARU_INGEST_UNTIL_SLOT")]
    ingest_until_slot: Option<u64>,

    /// Ingest at most the given number of blocks.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "INT", env = "AMARU_INGEST_MAXIMUM_BLOCKS")]
    ingest_maximum_blocks: Option<usize>,
}

/// Load archives containing blocks and sort them based on file name (filesystem access doesn't guarantee any ordering)
#[expect(clippy::panic)]
fn list_archive_names(network: NetworkName) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut archives: Vec<_> = fs::read_dir(format!("{}/blocks", default_data_dir(network)))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("gz") {
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

    fn extract_slot(s: &str) -> Option<u32> {
        s.split_once(".")
            .and_then(|(prefix, _)| prefix.parse::<u32>().ok())
    }

    archives.sort_by(|a, b| {
        let a_num = extract_slot(a).unwrap_or_else(|| panic!("Invalid archive name format: {}", a));
        let b_num = extract_slot(b).unwrap_or_else(|| panic!("Invalid archive name format: {}", b));
        a_num.cmp(&b_num)
    });
    Ok(archives)
}

fn load_archive(
    network: NetworkName,
    archive_path: &str,
) -> Result<Archive<GzDecoder<File>>, Box<dyn std::error::Error>> {
    let file = fs::File::open(format!(
        "{}/blocks/{}",
        default_data_dir(network),
        archive_path
    ))?;
    let gz = GzDecoder::new(file);
    Ok(Archive::new(gz))
}

fn create_praos_chain_store(
    global_parameters: GlobalParameters,
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    era_history: &EraHistory,
) -> PraosChainStore<BlockHeader> {
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
            let (slot_str, hash_str) = file_name
                .strip_suffix(".cbor")
                .ok_or_else(|| anyhow!("Missing .cbor suffix in file: {}", file_name))?
                .split_once('.')
                .ok_or_else(|| {
                    anyhow!(
                        "Invalid filename format (expected slot.hash.cbor): {}",
                        file_name
                    )
                })?;
            let slot = slot_str
                .parse::<u64>()
                .map_err(|e| anyhow!("Failed to parse slot from '{}': {}", slot_str, e))?;
            let hash = Hash::from_str(hash_str)
                .map_err(|e| anyhow!("Failed to decode hash from '{}': {}", hash_str, e))?;
            let point = Point::Specific(slot.into(), hash);
            let mut block_data = Vec::new();
            entry.read_to_end(&mut block_data)?;
            entries_with_keys.push((point, RawBlock::from(&*block_data)));
        }
    }

    // Sort by numeric key
    entries_with_keys.sort_by_key(|(num, _)| *num);
    Ok(entries_with_keys)
}

/// Process blocks as if they were processed by the full node
/// Particularly all on disk side-effects are performed
/// Blocks are assumed valid; no validation error should happen
#[allow(clippy::unwrap_used)]
async fn process_block(
    chain_store: &Arc<dyn ChainStore<BlockHeader>>,
    praos_chain_store: &PraosChainStore<BlockHeader>,
    consensus_parameters: Arc<ConsensusParameters>,
    block_validator: &BlockValidator<RocksDB, RocksDBHistoricalStores>,
    point: Point,
    raw_block: RawBlock,
) -> Result<(), Box<dyn std::error::Error>> {
    let network_block = NetworkBlock::try_from(raw_block)?;
    let block = network_block.decode_block()?;
    let block_header = BlockHeader::from(&block.header);
    chain_store.store_header(&block_header)?;
    chain_store.store_block(&point.hash(), &network_block.raw_block())?;
    let epoch_nonce = praos_chain_store.evolve_nonce(&block_header)?;

    // Verify block headers
    header::assert_all(
        consensus_parameters,
        block_header.header(),
        to_cbor(&block_header.header_body()).as_slice(),
        Arc::new(
            block_validator
                .state
                .lock()
                .unwrap()
                .view_stake_distribution(),
        ),
        &epoch_nonce.active,
    )
    .and_then(|assertions| assertions.into_par_iter().try_for_each(|assert| assert()))?;

    // Verify block content
    block_validator
        .roll_forward_block(&point, block)
        .await
        .map_err(|err| anyhow!("Error processing block at point {:?}: {:?}", point, err))?
        .map_err(|err| anyhow!("Error processing block at point {:?}: {:?}", point, err))?;

    Ok(())
}

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
    let consensus_parameters = Arc::new(ConsensusParameters::new(
        global_parameters.clone(),
        era_history,
        Default::default(),
    ));
    let block_validator = new_block_validator(network, ledger_dir)?;
    let tip = block_validator.get_tip();
    let chain_store: Arc<dyn ChainStore<BlockHeader>> =
        Arc::new(RocksDBStore::open(&RocksDbConfig::new(chain_dir))?);
    let praos_chain_store =
        create_praos_chain_store(global_parameters.clone(), chain_store.clone(), era_history);

    // Collect .tar.gz files
    let archive_names = list_archive_names(network)?;

    let mut processed = 0;
    let before = Instant::now();

    'archives: for archive_name in &archive_names {
        let mut archive = load_archive(network, archive_name)?;
        for (point, raw_block) in load_blocks(&mut archive)
            .await?
            .into_iter()
            // Do not process points already in the ledger
            .filter(|(point, _)| point.slot_or_default() > tip.slot_or_default())
        {
            if let Some(until) = args.ingest_until_slot
                && point.slot_or_default() > until.into()
            {
                break 'archives;
            }

            process_block(
                &chain_store,
                &praos_chain_store,
                consensus_parameters.clone(),
                &block_validator,
                point,
                raw_block,
            )
            .await?;

            processed += 1;

            if let Some(max) = args.ingest_maximum_blocks
                && processed >= max
            {
                break 'archives;
            }
        }
    }

    let duration = Instant::now().saturating_duration_since(before);
    let processed_per_seconds = processed as u64 / duration.as_secs().max(1);
    info!(
        processed_per_seconds,
        processed,
        duration = duration.as_secs(),
        "Finished processing blocks"
    );

    Ok(())
}
