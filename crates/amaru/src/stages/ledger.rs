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

use crate::stages::config::{Config, StoreType};
use amaru_kernel::{GlobalParameters, Point};
use amaru_ledger::block_validator::BlockValidator;
use amaru_ouroboros_traits::{CanValidateBlocks, HasStakeDistribution};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_slot_arithmetic::EraHistory;
use amaru_stores::in_memory::MemoryStore;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores};
use anyhow::anyhow;
use std::sync::Arc;

/// Representation of the ledger as used by the consensus stages.
/// It is either implemented in memory for tests or on disk for production.
pub enum Ledger {
    InMemLedger(BlockValidator<MemoryStore, MemoryStore>),
    OnDiskLedger(BlockValidator<RocksDB, RocksDBHistoricalStores>),
}

impl Ledger {
    pub fn new(
        config: &Config,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
    ) -> anyhow::Result<Ledger> {
        let vm_eval_pool = ArenaPool::new(
            config.ledger_vm_alloc_arena_count,
            config.ledger_vm_alloc_arena_size,
        );

        match &config.ledger_store {
            StoreType::InMem(store) => {
                let ledger = BlockValidator::new(
                    store.clone(),
                    store.clone(),
                    vm_eval_pool,
                    config.network,
                    era_history,
                    global_parameters,
                )?;
                Ok(Ledger::InMemLedger(ledger))
            }
            StoreType::RocksDb(rocks_db_config) => {
                let ledger = BlockValidator::new(
                    RocksDB::new(rocks_db_config)?,
                    RocksDBHistoricalStores::new(
                        rocks_db_config,
                        u64::from(config.max_extra_ledger_snapshots),
                    ),
                    vm_eval_pool,
                    config.network,
                    era_history,
                    global_parameters,
                )?;
                Ok(Ledger::OnDiskLedger(ledger))
            }
        }
    }

    /// Return the current ledger tip.
    pub fn get_tip(&self) -> Point {
        match self {
            Ledger::InMemLedger(stage) => stage.get_tip(),
            Ledger::OnDiskLedger(stage) => stage.get_tip(),
        }
    }

    /// Return the current stake distribution.
    /// It used to validate header nonces.
    pub fn get_stake_distribution(&self) -> anyhow::Result<Arc<dyn HasStakeDistribution>> {
        match self {
            Ledger::InMemLedger(stage) => {
                let state = stage.state.lock().map_err(|e| anyhow!(format!("{e:?}")))?;
                Ok(Arc::new(state.view_stake_distribution()))
            }
            Ledger::OnDiskLedger(stage) => {
                let state = stage.state.lock().map_err(|e| anyhow!(format!("{e:?}")))?;
                Ok(Arc::new(state.view_stake_distribution()))
            }
        }
    }

    /// Return the ledger as a capability for validating blocks.
    pub fn get_block_validation(self) -> Arc<dyn CanValidateBlocks + Send + Sync> {
        match self {
            Ledger::InMemLedger(stage) => Arc::new(stage),
            Ledger::OnDiskLedger(stage) => Arc::new(stage),
        }
    }
}
