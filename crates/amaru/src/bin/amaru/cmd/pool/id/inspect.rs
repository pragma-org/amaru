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

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::PoolId;
use amaru_ouroboros::load_cold_verification_key;
use anyhow::Context;

/// Derive the pool ID from a cold verification key.
#[derive(Debug, clap::Args)]
pub struct InspectArgs {
    /// Pool cold verification key (Ed25519)
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    verification_key: PathBuf,
}

impl InspectArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    #[expect(clippy::print_stdout)]
    pub fn run(self) -> anyhow::Result<()> {
        let cold = load_cold_verification_key(&self.verification_key)
            .with_context(|| format!("failed to load cold verification key: {}", self.verification_key.display()))?;

        println!("{}", PoolId::from(cold));

        Ok(())
    }
}
