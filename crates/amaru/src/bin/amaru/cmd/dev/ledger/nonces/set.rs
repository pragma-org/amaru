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

use std::path::PathBuf;

use amaru::{
    default_chain_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{Epoch, HeaderHash, NetworkName, Nonce, parse_nonce};
use amaru_observability::info;
use amaru_ouroboros::{Nonces, WriteChainStore};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;

use crate::cmd::PointOrHash;

fn parse_nonce_arg(s: &str) -> Result<Nonce, String> {
    parse_nonce(s)
}

fn parse_header_hash(s: &str) -> Result<HeaderHash, String> {
    s.parse::<HeaderHash>().map_err(|e| e.to_string())
}

#[derive(Debug, Parser)]
pub struct Args {
    /// The block hash or point to set nonces for.
    #[arg(value_name = amaru::value_names::POINT_OR_HASH)]
    block: PointOrHash,

    /// The active nonce (hex-encoded 32 bytes).
    #[arg(long, value_parser = parse_nonce_arg)]
    active: Nonce,

    /// The evolving nonce (hex-encoded 32 bytes).
    #[arg(long, value_parser = parse_nonce_arg)]
    evolving: Nonce,

    /// The candidate nonce (hex-encoded 32 bytes).
    #[arg(long, value_parser = parse_nonce_arg)]
    candidate: Nonce,

    /// The tail header hash (hex-encoded 32 bytes).
    #[arg(long, value_parser = parse_header_hash)]
    tail: HeaderHash,

    /// The epoch number.
    #[arg(long)]
    epoch: Epoch,

    /// The path to the chain database.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Network of the underlying chain database.
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

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        cli::dev::RUN,
        command = "dev ledger nonces set",
        network = args.network,
        chain_dir = chain_dir.to_string_lossy(),
        block = args.block.to_string()
    );

    let db = RocksDBStore::open(&RocksDbConfig::new(chain_dir))?;

    let nonces = Nonces {
        active: args.active,
        evolving: args.evolving,
        candidate: args.candidate,
        tail: args.tail,
        epoch: args.epoch,
    };

    db.put_nonces(&args.block, &nonces)?;

    println!("Nonces set for {}", *args.block);

    Ok(())
}
