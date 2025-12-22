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

use amaru::{
    DEFAULT_NETWORK,
    bootstrap::{ImportError, import_snapshots, import_snapshots_from_directory},
    default_ledger_dir,
};
use amaru_kernel::network::NetworkName;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the CBOR snapshot. The snapshot can be obtained from the Haskell
    /// cardano-node, using the `DebugEpochState` command, serialised as CBOR.
    ///
    /// The snapshot must be named after the point on-chain it is reflecting, as
    ///
    /// `  {SLOT}.{BLOCK_HEADER_HASH}.cbor`
    ///
    /// For example:
    ///
    ///   68774372.36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor
    ///
    /// Can be repeated multiple times for multiple snapshots.
    #[arg(long, value_name = "FILE", env = "AMARU_SNAPSHOT", verbatim_doc_comment, num_args(0..))]
    snapshot: Vec<PathBuf>,

    /// Path to a directory containing multiple CBOR snapshots to import.
    ///
    /// If not provided, defaults to a per-network snapshots directory based on the network name.
    #[arg(long, value_name = "DIR", env = "AMARU_SNAPSHOTS_DIR")]
    snapshot_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_LEDGER_DIR")]
    ledger_dir: Option<PathBuf>,

    /// Network the snapshots are imported from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet_<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(network = %args.network, ledger_dir=%ledger_dir.to_string_lossy(), snapshot=?args.snapshot, snapshot_dir=%args.snapshot_dir.clone().unwrap_or_default().to_string_lossy(),
          "Running command import-ledger-state",
    );

    if !args.snapshot.is_empty() {
        import_snapshots(args.network, &args.snapshot, &ledger_dir).await
    } else if let Some(snapshot_dir) = args.snapshot_dir {
        import_snapshots_from_directory(args.network, &ledger_dir, &snapshot_dir).await
    } else {
        Err(ImportError::IncorrectUsage.into())
    }
}
