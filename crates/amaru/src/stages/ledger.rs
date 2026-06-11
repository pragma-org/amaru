// Copyright 2024 PRAGMA
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

use std::sync::{Arc, Mutex};

use amaru_kernel::{EraHistory, GlobalParameters, ORIGIN_HASH, Point, Tip};
use amaru_ledger::{block_validator::BlockValidator, header_validator::HeaderValidator, peers_data::PeersData, state};
use amaru_ouroboros::{CanValidateBlocks, CanValidateHeaders, CanValidateTxs, ChainStore, HasPeersData};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores};
use anyhow::anyhow;

use crate::stages::config::LedgerConfig;

/// Representation of the ledger as used by the consensus stages.
pub struct Ledger {
    state: Arc<Mutex<state::State<RocksDB, RocksDBHistoricalStores>>>,
    chain_store: Arc<dyn ChainStore>,
    block_validator: BlockValidator<RocksDB, RocksDBHistoricalStores>,
    header_validator: HeaderValidator<RocksDB, RocksDBHistoricalStores>,
    peers_data: PeersData<RocksDB, RocksDBHistoricalStores>,
}

impl Ledger {
    pub fn new(config: &LedgerConfig, chain_store: Arc<dyn ChainStore>) -> anyhow::Result<Ledger> {
        let state = Arc::new(Mutex::new(Self::make_state(config)?));

        Ok(Ledger {
            state: state.clone(),
            chain_store: chain_store.clone(),
            block_validator: BlockValidator::new(
                state.clone(),
                ArenaPool::new(config.ledger_vm_alloc_arena_count, config.ledger_vm_alloc_arena_size),
            )?,
            header_validator: HeaderValidator::new(
                state.clone(),
                Arc::new(config.consensus_parameters()),
                chain_store.clone(),
            )?,
            peers_data: PeersData::new(state.clone())?,
        })
    }

    #[expect(clippy::panic)]
    fn make_state(config: &LedgerConfig) -> anyhow::Result<state::State<RocksDB, RocksDBHistoricalStores>> {
        let era_history: &EraHistory =
            config.network.as_era_history().unwrap_or_else(|| panic!("missing default EraHistory for network"));
        let global_parameters: &GlobalParameters = config
            .network
            .as_global_parameters()
            .unwrap_or_else(|| panic!("missing default GlobalParameters for network"));

        let store = RocksDB::new(&config.ledger_store)?;
        let snapshots =
            RocksDBHistoricalStores::new(&config.ledger_store, u64::from(config.max_extra_ledger_snapshots));
        Ok(state::State::new(store, snapshots, config.network, era_history.clone(), global_parameters.clone())?)
    }

    /// Return the ledger as a capability for validating blocks.
    pub fn get_block_validation(&self) -> Arc<dyn CanValidateBlocks + Send + Sync> {
        Arc::new(self.block_validator.clone())
    }

    pub fn get_tx_validation(&self) -> Arc<dyn CanValidateTxs + Send + Sync> {
        Arc::new(self.block_validator.clone())
    }

    /// Return the ledger as a capability for validating headers.
    pub fn get_header_validation(&self) -> Arc<dyn CanValidateHeaders + Send + Sync> {
        Arc::new(self.header_validator.clone())
    }

    pub fn get_peers_data(&self) -> Arc<dyn HasPeersData + Send + Sync> {
        Arc::new(self.peers_data.clone())
    }

    #[expect(clippy::unwrap_used)]
    pub fn get_tip(&self) -> Point {
        let state = self.state.lock().unwrap();
        state.tip().into_owned()
    }

    #[expect(clippy::unwrap_used)]
    pub fn initialize_chain_store(&self) -> anyhow::Result<Tip> {
        let state = self.state.lock().unwrap();
        let ledger_tip = state.tip();
        tracing::info!(
            tip.hash = %ledger_tip.hash(),
            tip.slot = u64::from(ledger_tip.slot_or_default()),
            "initialize_chain_store"
        );

        let anchor_hash = self.chain_store.get_anchor_hash();

        // This corresponds to a bootstrap, we need to correctly initialize the chain store
        if anchor_hash == ORIGIN_HASH {
            tracing::info!(anchor = %ledger_tip, "first initialization - setting anchor and best chain");
            self.chain_store.set_anchor_hash(&ledger_tip.hash())?;
            self.chain_store.set_block_valid(&ledger_tip.hash(), true)?;
            self.chain_store.roll_forward_chain(&ledger_tip)?;
        };

        // Check that the ledger tip can be retrieved from a stored header and return it
        self.chain_store.load_tip(&ledger_tip.hash()).ok_or(anyhow!("ledger tip header not found"))
    }
}
