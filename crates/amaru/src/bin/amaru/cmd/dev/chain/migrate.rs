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

use std::path::PathBuf;

use amaru::{
    default_chain_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use amaru_observability::{error, info, info_span};
use amaru_ouroboros::StoreError;
use amaru_stores::rocksdb::{
    RocksDbConfig,
    consensus::{RocksDBStore, check_db_version, migrate_db, util::open_db},
};
use clap::Parser;

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
    )]
    network: NetworkName,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        cli::dev::RUN,
        command = "dev chain migrate",
        network = args.network,
        chain_dir = chain_dir.to_string_lossy()
    );

    let config = RocksDbConfig::new(chain_dir.clone());
    let config_dir = config.dir.display().to_string();

    Ok(info_span!(consensus::chain_db::OPEN, path = config_dir).in_scope(|| {
        let (basedir, db) = open_db(&config)?;
        let store = RocksDBStore { db, basedir };
        match check_db_version(&store) {
            Ok(()) => {
                info!(cli::dev::chain::MIGRATION_NOT_NEEDED);
                Ok(())
            }
            Err(StoreError::IncompatibleChainStoreVersions { stored, current }) => {
                info_span!(consensus::chain_db_migration::EXECUTE, from = stored, to = current)
                    .in_scope(|| migrate_db(&store))?;
                Ok(())
            }
            Err(e) => {
                error!(cli::dev::chain::OPEN_FAILED, error = e.to_string());
                Err(Box::new(e))
            }
        }
    })?)
}
