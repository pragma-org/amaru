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

use amaru::{DEFAULT_NETWORK, default_chain_dir};
use amaru_kernel::NetworkName;
use amaru_ouroboros::StoreError;
use amaru_stores::rocksdb::{
    RocksDbConfig,
    consensus::{check_db_version, migrate_db, util::open_db},
};
use clap::Parser;
use std::{error::Error, path::PathBuf};
use tracing::{error, info, info_span};

#[derive(Debug, Parser)]
pub struct Args {
    /// The path to the chain database to migrate
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR
    )]
    chain_dir: Option<PathBuf>,

    /// Underlying network of the database to migrate
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    let config = RocksDbConfig::new(chain_dir.clone());

    info!(
        _command = "migrate-chain-db",
        chain_dir = %chain_dir.to_string_lossy(),
        network = %args.network,
        "running",
    );

    Ok(
        info_span!("opening chain db", path = %config.dir.display()).in_scope(|| {
            let (_, db) = open_db(&config)?;
            match check_db_version(&db) {
                Ok(()) => {
                    info!("already up to date, no migration needed.");
                    Ok(())
                }
                Err(StoreError::IncompatibleChainStoreVersions { stored, current }) => {
                    info_span!("migrating database", from = stored, to = current)
                        .in_scope(|| migrate_db(&db))?;
                    Ok(())
                }
                Err(e) => {
                    error!(error = %e, "failed to open database");
                    Err(Box::new(e))
                }
            }
        })?,
    )
}
