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

use std::path::{Path, PathBuf};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{VerificationKey, cbor};
use amaru_ouroboros::{credentials::generate_cold_key_pair, envelope::HasEnvelope};
use anyhow::Context;
use tempfile::NamedTempFile;

use super::{ensure_absent, prepare_file, publish_new};

/// Manage pool cold keys and certificate issue counters offline.
#[derive(Debug, clap::Subcommand)]
pub enum ColdCommand {
    Create(CreateArgs),
}

impl ColdCommand {
    pub fn into_runnable(self) -> Runnable {
        match self {
            Self::Create(cmd) => cmd.into_runnable(),
        }
    }
}

/// The next certificate number, bound to the cold verification key that owns it.
pub(super) struct IssueCounter {
    pub next: u64,
    pub cold: VerificationKey,
}

impl HasEnvelope for IssueCounter {
    const ACCEPTED_TYPES: &'static [&'static str] = &["NodeOperationalCertificateIssueCounter"];
}

impl<'d, C: cbor::HasProtocolVersion> cbor::Decode<'d, C> for IssueCounter {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self { next: d.u64()?, cold: d.decode_with(ctx)? })
        })
    }
}

pub(super) fn prepare_counter(counter: &IssueCounter, path: &Path) -> anyhow::Result<NamedTempFile> {
    let mut encoder = cbor::Encoder::new(Vec::new());
    encoder.array(2)?.u64(counter.next)?.bytes(counter.cold.as_slice())?;
    prepare_file(path, &encoder.into_writer())
}

/// Generate raw cold keys and a binary CBOR issue counter starting at zero.
///
/// Run offline. The signing key is unencrypted; keep it and the counter on the cold machine.
/// Existing files are never replaced.
#[derive(Debug, clap::Args)]
pub struct CreateArgs {
    /// Destination for the binary CBOR pair [next issue number, cold verification key].
    #[arg(long, env = "AMARU_POOL_COUNTER", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    counter: PathBuf,

    /// Destination for the raw 32-byte cold signing seed.
    #[arg(long, env = "AMARU_POOL_COLD_SIGNING_KEY", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    signing_key: PathBuf,

    /// Destination for the raw 32-byte cold verification key.
    #[arg(long, env = "AMARU_POOL_COLD_VERIFICATION_KEY", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    verification_key: PathBuf,
}

impl CreateArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    pub fn run(self) -> anyhow::Result<()> {
        let paths = [self.signing_key.as_path(), self.verification_key.as_path(), self.counter.as_path()];
        paths.into_iter().try_for_each(ensure_absent)?;
        let (secret, public) = generate_cold_key_pair().context("could not generate cold key")?;
        let signing = prepare_file(&self.signing_key, secret.as_bytes())?;
        let verification = prepare_file(&self.verification_key, public.as_slice())?;
        let counter = prepare_counter(&IssueCounter { next: 0, cold: public }, &self.counter)?;
        publish_new(paths.into_iter().zip([signing, verification, counter]))
    }
}
