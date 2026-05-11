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

use std::sync::Arc;

use amaru_kernel::{EraHistory, GlobalParameters, Point};
use amaru_ledger::block_validator::BlockValidator;
use amaru_ouroboros::{CanValidateBlocks, CanValidateTxs, HasStakeDistribution, HasStakePools};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores};

use crate::stages::config::Config;

/// Representation of the ledger as used by the consensus stages.
pub struct Ledger(BlockValidator<RocksDB, RocksDBHistoricalStores>);

impl Ledger {
    pub fn new(
        config: &Config,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
    ) -> anyhow::Result<Ledger> {
        let vm_eval_pool = ArenaPool::new(config.ledger_vm_alloc_arena_count, config.ledger_vm_alloc_arena_size);

        let ledger = BlockValidator::new(
            RocksDB::new(&config.ledger_store)?,
            RocksDBHistoricalStores::new(&config.ledger_store, u64::from(config.max_extra_ledger_snapshots)),
            vm_eval_pool,
            config.network,
            era_history,
            global_parameters,
        )?;
        Ok(Ledger(ledger))
    }

    /// Return the current ledger tip.
    pub fn get_tip(&self) -> Point {
        self.0.get_tip()
    }

    /// Return the current stake distribution.
    /// It used to validate header nonces.
    pub fn get_stake_distribution(&self) -> anyhow::Result<Arc<dyn HasStakeDistribution>> {
        let state = self.0.state.lock().map_err(|e| anyhow::anyhow!("{:?}", e))?;
        Ok(Arc::new(state.view_stake_distribution()))
    }

    /// Return the ledger as a capability for validating blocks.
    pub fn get_block_validation(&self) -> Arc<dyn CanValidateBlocks + Send + Sync> {
        Arc::new(self.0.clone())
    }

    pub fn get_stake_pools(&self) -> Arc<dyn HasStakePools + Send + Sync> {
        Arc::new(self.0.clone())
    }

    pub fn get_tx_validation(&self) -> Arc<dyn CanValidateTxs + Send + Sync> {
        Arc::new(self.0.clone())
    }
}
