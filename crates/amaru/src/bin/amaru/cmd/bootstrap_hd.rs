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
    error::Error,
    fs::{remove_dir_all, remove_file},
    path::{Path, PathBuf},
};

use amaru::{DEFAULT_NETWORK, bootstrap::import_node_ledger_snapshot, default_chain_dir, default_ledger_dir};
use amaru_kernel::NetworkName;
use clap::{ArgAction, Parser};
use tracing::{info, warn};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the chain on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Forcefully erase and overwrite existing chain and ledger databases.
    #[arg(
        short,
        long,
        action = ArgAction::SetTrue,
        default_value_t = false,
    )]
    force: bool,

    /// Path of the ledger on-disk storage (Amaru's output database).
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// Network the snapshot belongs to.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// Paths to one or more cardano-node UTxOHD ledger snapshot directories.
    ///
    /// Supply multiple directories as a comma-separated list (or repeat the flag).
    /// Each entry is the slot-numbered directory written by the cardano-node's LedgerDB,
    /// for example `db/ledger/119183041`. Each directory must contain:
    ///
    ///   <dir>/state        — CBOR ledger state (NewEpochState, no UTxO)
    ///   <dir>/tables/tvar  — CBOR UTxO table
    ///
    /// When more than one directory is provided they are imported in the order given,
    /// so list them from oldest to newest.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        value_delimiter = ',',
        verbatim_doc_comment,
        required = true,
    )]
    node_snapshot_dirs: Vec<PathBuf>,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        _command = "bootstrap-hd",
        chain_dir = %chain_dir.to_string_lossy(),
        force = %args.force,
        ledger_dir = %ledger_dir.to_string_lossy(),
        network = %network,
        node_snapshot_dirs = ?args.node_snapshot_dirs,
        "running",
    );

    guard_output_dir(&ledger_dir, args.force, "ledger")?;
    guard_output_dir(&chain_dir, args.force, "chain")?;

    import_node_ledger_snapshot(network, ledger_dir, chain_dir, args.node_snapshot_dirs).await
}

fn guard_output_dir(dir: &Path, force: bool, kind: &str) -> Result<(), Box<dyn Error>> {
    if !path_has_data(dir)? {
        return Ok(());
    }

    if !force {
        warn!(dir=%dir.to_string_lossy(), kind, "output directory already contains data; use --force to remove it first");
        return Err(format!("{kind} directory is not empty: {}", dir.display()).into());
    }

    info!(dir=%dir.to_string_lossy(), kind, "removing existing output directory due to --force");
    if dir.is_dir() {
        remove_dir_all(dir)?;
    } else {
        remove_file(dir)?;
    }
    Ok(())
}

fn path_has_data(dir: &Path) -> Result<bool, Box<dyn Error>> {
    if !dir.exists() {
        return Ok(false);
    }

    if !dir.is_dir() {
        return Ok(true);
    }

    Ok(std::fs::read_dir(dir)?.next().is_some())
}
