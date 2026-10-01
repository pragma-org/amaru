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

use std::{path::Path, process::ExitCode};

use anyhow::{Context, anyhow};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("amaru KES signer: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or_else(|| anyhow!("missing KES key path"))?;
    let start = args.next().ok_or_else(|| anyhow!("missing KES start period"))?;
    let max = args.next().ok_or_else(|| anyhow!("missing KES evolution limit"))?;
    anyhow::ensure!(args.next().is_none(), "unexpected KES signer argument");
    let start = start.to_string_lossy().parse::<u64>().context("invalid KES start period")?;
    let max = max.to_string_lossy().parse::<u64>().context("invalid KES evolution limit")?;
    amaru_ouroboros::process_credentials::run_kes_signer(Path::new(&path), start.into(), max)
        .map_err(|error| anyhow!(error.to_string()))
}
