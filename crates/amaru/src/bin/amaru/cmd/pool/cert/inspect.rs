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
use amaru_kernel::{PoolId, protocol_version};
use amaru_ouroboros::load_operational_certificate;
use anyhow::Context;

/// Inspect an operational certificate and display its fields.
#[derive(Debug, clap::Args)]
pub struct InspectArgs {
    /// Operational certificate to inspect.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    cert: PathBuf,
}

impl InspectArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    #[expect(clippy::print_stdout)]
    pub fn run(self) -> anyhow::Result<()> {
        let (cert, cold) = load_operational_certificate(&self.cert, protocol_version::DEFAULT)
            .with_context(|| format!("failed to load operational certificate: {}", self.cert.display()))?;

        let pool_id = PoolId::from(cold);

        let info = serde_json::json!({
            "pool_id": pool_id.to_string(),
            "sequence_number": cert.operational_cert_sequence_number,
            "kes_period": u64::from(cert.operational_cert_kes_period),
            "kes_verification_key": cert.operational_cert_kes_verification_key.to_string(),
        });

        println!("{}", serde_json::to_string_pretty(&info)?);

        Ok(())
    }
}
