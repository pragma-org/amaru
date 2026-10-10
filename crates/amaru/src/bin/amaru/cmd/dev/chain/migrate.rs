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

use amaru::lifecycle::{Runnable, RuntimeKind};
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
    #[command(flatten)]
    db_chain: amaru::args::DbChain,

    #[command(flatten)]
    network: amaru::args::Network,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);
    let db_chain = args.db_chain.into_path_buf(network);

    info!(cli::dev::chain::MIGRATE, db_chain = db_chain.to_string_lossy(), network);

    let config = RocksDbConfig::new(db_chain.clone());
    let config_dir = config.dir.display().to_string();

    Ok(info_span!(consensus::db_chain::OPEN, path = config_dir).in_scope(|| {
        let (basedir, db) = open_db(&config)?;
        let store = RocksDBStore { db, basedir };
        match check_db_version(&store) {
            Ok(()) => {
                info!(cli::dev::chain::MIGRATION_NOT_NEEDED);
                Ok(())
            }
            Err(StoreError::IncompatibleChainStoreVersions { stored, current }) => {
                info_span!(consensus::db_chain_migration::EXECUTE, from = stored, to = current)
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
