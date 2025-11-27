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

use amaru_kernel::{EraHistory, network::NetworkName, protocol_parameters::GlobalParameters};
use amaru_ledger::block_validator::BlockValidator;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};
use std::{error::Error, path::PathBuf};

pub(crate) mod mithril;
pub(crate) mod sync;

pub fn new_block_validator(
    network: NetworkName,
    ledger_dir: PathBuf,
) -> Result<BlockValidator<RocksDB, RocksDBHistoricalStores>, Box<dyn Error>> {
    let era_history: &EraHistory = network.into();
    let global_parameters: &GlobalParameters = network.into();
    let config = RocksDbConfig::new(ledger_dir);
    let store = RocksDBHistoricalStores::new(&config, 2);
    let block_validator = BlockValidator::new(
        RocksDB::new(&config)?,
        store,
        network,
        era_history.clone(),
        global_parameters.clone(),
    )?;
    Ok(block_validator)
}
