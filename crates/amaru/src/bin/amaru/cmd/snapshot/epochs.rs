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

use std::io::{self, Write};

use amaru::{
    aws::{DEFAULT_PUBLIC_URL, S3Config},
    bootstrap::bootstrap_epochs,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// Network to query for published bootstrap snapshots.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,

    /// Public CDN base URL for anonymous snapshot discovery.
    #[arg(
        long,
        value_name = amaru::value_names::URL,
        env = "AMARU_S3_PUBLIC_URL",
        default_value = DEFAULT_PUBLIC_URL,
    )]
    s3_public_url: String,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let s3_config = S3Config { public_url: args.s3_public_url, ..S3Config::default() };
    let epochs = bootstrap_epochs(args.network, s3_config).await?;
    let mut stdout = io::stdout().lock();
    for epoch in epochs {
        writeln!(stdout, "{epoch}")?;
    }
    Ok(())
}
