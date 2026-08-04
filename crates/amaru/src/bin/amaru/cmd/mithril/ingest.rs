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

use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Instant,
};

use amaru_consensus::{block_validator::BlockValidator, store::PraosChainStore};
use amaru_kernel::{
    Block, BlockHeader, ConsensusParameters, EraHistory, GlobalParameters, IsHeader, NetworkName, Point, RawBlock,
    cardano::network_block::NetworkBlock, to_cbor,
};
use amaru_node::stages::{
    build_node::{make_block_validator, make_state},
    config::LedgerConfig,
};
use amaru_ouroboros::{ChainStore, PoolSummaries, Praos, can_validate_blocks::CanValidateBlocks, praos::header};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use pallas_hardano::storage::immutable::read_blocks_from_point;
use pallas_network::miniprotocols::Point as PallasPoint;
use rayon::prelude::*;
use tracing::info;

fn to_pallas_point(point: Point) -> PallasPoint {
    match point {
        Point::Origin => PallasPoint::Origin,
        Point::Specific(slot, hash) => PallasPoint::Specific(slot.as_u64(), hash.to_vec()),
    }
}

fn create_praos_chain_store(
    global_parameters: GlobalParameters,
    chain_store: Arc<dyn ChainStore>,
    era_history: &EraHistory,
) -> PraosChainStore {
    let consensus_parameters = Arc::new(ConsensusParameters::new(global_parameters, era_history));
    PraosChainStore::new(consensus_parameters, chain_store)
}

/// Process blocks as if they were processed by the full node
/// Particularly all on disk side-effects are performed
/// Blocks are assumed valid; no validation error should happen
#[expect(clippy::unwrap_used)]
#[expect(clippy::too_many_arguments)]
#[expect(clippy::result_large_err)]
async fn process_block(
    chain_store: &Arc<dyn ChainStore>,
    praos_chain_store: &PraosChainStore,
    consensus_parameters: Arc<ConsensusParameters>,
    block_validator: &BlockValidator<RocksDB, RocksDBHistoricalStores>,
    pool_summaries: &RwLock<PoolSummaries>,
    era_history: &EraHistory,
    raw_block: &RawBlock,
    block: Block,
    block_header: BlockHeader,
) -> Result<(), Box<dyn std::error::Error>> {
    let point = block_header.point();
    chain_store.store_block(&point.hash(), raw_block)?;
    let nonces = praos_chain_store.evolve_nonce(&block_header)?;

    {
        let summaries = pool_summaries.read().unwrap();
        let pool_id = block_header.pool_id();
        let last_opcert_sequence_number = chain_store.get_latest_opcert_sequence_number(&pool_id, &block_header)?;
        let pool_summary = summaries
            .get_pool(block_header.slot(), &pool_id, era_history)?
            .ok_or_else(|| anyhow!("unknown pool: {pool_id:?}"))?;
        header::assert_all(
            consensus_parameters,
            block_header.header(),
            to_cbor(&block_header.header_body()).as_slice(),
            last_opcert_sequence_number,
            &pool_summary,
            &nonces.active,
        )
        .and_then(|assertions| assertions.into_par_iter().try_for_each(|assert| assert()))?;
    }

    chain_store.store_validated_header(&block_header, &nonces)?;

    // Verify block content
    block_validator
        .roll_forward_block(&point, block)
        .await
        .map_err(|err| anyhow!("Error processing block at point {:?}: {:?}", point, err))?
        .map_err(|err| anyhow!("Error processing block at point {:?}: {:?}", point, err))?;

    Ok(())
}

pub(super) async fn run(
    network: NetworkName,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    immutable_dir: PathBuf,
    ingest_until_slot: Option<u64>,
    ingest_maximum_blocks: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let era_history: &EraHistory =
        network.as_era_history().ok_or_else(|| anyhow!("missing default EraHistory for network: {network}"))?;

    let global_parameters: &GlobalParameters = network
        .as_global_parameters()
        .ok_or_else(|| anyhow!("missing default GlobalParameters for network: {network}"))?;

    let consensus_parameters = Arc::new(ConsensusParameters::new(global_parameters.clone(), era_history));

    let chain_store: Arc<dyn ChainStore> = Arc::new(RocksDBStore::open(&RocksDbConfig::new(chain_dir))?);
    let praos_chain_store = create_praos_chain_store(global_parameters.clone(), chain_store.clone(), era_history);

    let ledger_config =
        LedgerConfig { ledger_store: RocksDbConfig::new(ledger_dir), network, ..LedgerConfig::default() };
    let state = make_state(&ledger_config, None)?;
    let tip = state.tip().into_owned();
    let pool_summaries = Arc::new(RwLock::new(state.pool_summaries()));
    let block_validator = make_block_validator(&ledger_config, state, chain_store.clone())?;
    {
        let pool_summaries = pool_summaries.clone();
        #[allow(clippy::unwrap_used)]
        block_validator
            .set_on_stake_dist_updated(Arc::new(move |summaries| *pool_summaries.write().unwrap() = summaries));
    }

    let mut processed = 0;
    let before = Instant::now();
    let skip_tip = usize::from(tip != Point::Origin);
    let blocks = read_blocks_from_point(&immutable_dir, to_pallas_point(tip))?.skip(skip_tip);

    for block in blocks {
        let block = block?;
        let raw_block = RawBlock::from(block.into_boxed_slice());
        let network_block = NetworkBlock::try_from(raw_block.clone())?;
        let block = network_block.decode_block()?;
        let block_header = BlockHeader::from(&block.header);
        let point = block_header.point();
        if let Some(until) = ingest_until_slot
            && point.slot_or_default() > until.into()
        {
            break;
        }

        process_block(
            &chain_store,
            &praos_chain_store,
            consensus_parameters.clone(),
            &block_validator,
            &pool_summaries,
            era_history,
            &raw_block,
            block,
            block_header,
        )
        .await?;

        processed += 1;

        if let Some(max) = ingest_maximum_blocks
            && processed >= max
        {
            break;
        }
    }

    let duration = Instant::now().saturating_duration_since(before);
    let duration_seconds = duration.as_secs_f64();
    let processed_per_seconds = processed as f64 / duration_seconds;
    info!(processed_per_seconds, processed, duration = duration_seconds, "Finished processing blocks");

    Ok(())
}
