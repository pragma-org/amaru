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
use amaru_kernel::MAINNET_GLOBAL_PARAMETERS;
use amaru_ouroboros::load_operational_certificate;
use anyhow::{Context, ensure};

/// Show the current KES period and how many periods remain before key rotation is required.
#[derive(Debug, clap::Args)]
pub struct PeriodArgs {
    /// Operational certificate used to determine the KES period at which the key was issued.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
    )]
    cert: PathBuf,

    /// Current slot number.
    #[arg(
        long,
        value_name = amaru::value_names::SLOT,
    )]
    slot: u64,

    /// Number of slots per KES period.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        default_value_t = MAINNET_GLOBAL_PARAMETERS.slots_per_kes_period,
    )]
    slots_per_kes_period: u64,

    /// Maximum number of KES evolutions before key rotation is required.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        default_value_t = MAINNET_GLOBAL_PARAMETERS.max_kes_evolution as u64,
    )]
    max_kes_evolutions: u64,
}

impl PeriodArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    #[expect(clippy::print_stdout)]
    pub fn run(self) -> anyhow::Result<()> {
        ensure!(self.slots_per_kes_period > 0, "--slots-per-kes-period must be greater than zero");

        let (cert, _) = load_operational_certificate(&self.cert)
            .with_context(|| format!("failed to load operational certificate: {}", self.cert.display()))?;

        let cert_kes_period = u64::from(cert.operational_cert_kes_period);
        let current_kes_period = self.slot / self.slots_per_kes_period;
        let periods_used = current_kes_period.saturating_sub(cert_kes_period);
        let periods_remaining = self.max_kes_evolutions.saturating_sub(periods_used);

        let info = serde_json::json!({
            "kes_period_current": current_kes_period,
            "kes_period_cert": cert_kes_period,
            "periods_used": periods_used,
            "periods_remaining": periods_remaining,
        });

        println!("{}", serde_json::to_string_pretty(&info)?);

        Ok(())
    }
}
