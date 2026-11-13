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
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{KesPeriod, OperationalCert, VerificationKey, cbor, protocol_version::PROTOCOL_VERSION_12};
use amaru_ouroboros::{
    credentials::{ColdSigningKey, load_cold_signing_key},
    envelope::{HasEnvelope, ReadEnvelopeError, read_cbor_or_envelope, read_key_file},
};
use anyhow::{Context, ensure};
use tempfile::NamedTempFile;

use super::super::{
    cold::{IssueCounter, prepare_counter},
    ensure_absent, prepare_file, sync_parent,
};

/// Create an operational certificate offline and advance its cold key's issue counter.
///
/// Reads raw cold and KES keys and a CBOR counter; legacy cardano-cli envelopes are also accepted.
/// Writes a binary CBOR certificate and counter. Existing certificates are never replaced.
/// A persistent COUNTER.lock file serializes Amaru issuers; do not remove it or issue with other
/// tools concurrently. The counter is advanced before publishing the certificate so an interrupted
/// issuance cannot reuse its number. A durable COUNTER.cert-recovery copy is saved first and removed
/// after publication. If it remains after an interruption, recover the certificate and check the
/// counter before removing it and issuing again. Use a directory only you can modify.
#[derive(Debug, clap::Args)]
pub struct CreateArgs {
    /// Destination for the binary CBOR pair [operational certificate, cold verification key].
    #[arg(long, env = "AMARU_POOL_CERT", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    cert: PathBuf,

    /// Cold signing key, as a raw 32-byte seed or an unencrypted cardano-cli text envelope.
    #[arg(long, env = "AMARU_POOL_COLD_SIGNING_KEY", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    cold_signing_key: PathBuf,

    /// Issue counter belonging to the cold key. Updated atomically on issuance.
    #[arg(long, env = "AMARU_POOL_COUNTER", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    counter: PathBuf,

    /// Absolute KES period at which the certificate starts; obtain it from the online node.
    #[arg(long, env = "AMARU_POOL_KES_PERIOD", value_name = amaru::value_names::UINT, display_order = 0)]
    kes_period: u64,

    /// KES verification key, as 32 raw bytes or a cardano-cli text envelope.
    #[arg(long, env = "AMARU_POOL_KES_VERIFICATION_KEY", value_name = amaru::value_names::FILEPATH, display_order = 0)]
    kes_verification_key: PathBuf,
}

impl CreateArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    pub fn run(self) -> anyhow::Result<()> {
        ensure_absent(&self.cert)?;
        let counter_path = self
            .counter
            .canonicalize()
            .with_context(|| format!("could not locate issue counter {}", self.counter.display()))?;
        let _lock = lock_counter(&counter_path)?;
        let recovery = recovery_path(&counter_path);
        ensure_absent(&recovery).with_context(|| {
            format!(
                "unfinished certificate issuance at {}; recover the certificate and check the issue counter before removing it and issuing again",
                recovery.display(),
            )
        })?;
        let mut ctx = PROTOCOL_VERSION_12;
        let counter = read_cbor_or_envelope(&counter_path, &mut ctx)
            .with_context(|| format!("could not read issue counter {}", counter_path.display()))?;
        let cold = load_cold_signing_key(&self.cold_signing_key)
            .with_context(|| format!("could not read cold signing key {}", self.cold_signing_key.display()))?;
        let hot = read_kes_verification_key(&self.kes_verification_key)?;
        let (certificate, next_counter) = issue_certificate(&cold, counter, hot, KesPeriod::from(self.kes_period))?;
        let staged_cert = prepare_file(&self.cert, &certificate)?;
        let staged_counter = prepare_counter(&next_counter, &counter_path)?;
        ensure_absent(&self.cert)?;
        advance_counter(&certificate, staged_counter, &counter_path, &recovery)?;
        publish_certificate(staged_cert, &self.cert, &recovery)
    }
}

/// Validate ownership and derive the certificate and next counter without changing either input.
fn issue_certificate(
    cold: &ColdSigningKey,
    counter: IssueCounter,
    hot: VerificationKey,
    period: KesPeriod,
) -> anyhow::Result<(Vec<u8>, IssueCounter)> {
    let public = cold.verification_key();
    ensure!(counter.cold == public, "issue counter does not belong to the cold signing key");
    let next = counter.next.checked_add(1).context("issue counter is exhausted")?;
    let message = [hot.as_slice(), &counter.next.to_be_bytes(), &u64::from(period).to_be_bytes()].concat();
    let certificate = OperationalCert {
        operational_cert_kes_verification_key: hot,
        operational_cert_sequence_number: counter.next,
        operational_cert_kes_period: period,
        operational_cert_sigma: cold.sign(&message),
    };
    let mut encoder = cbor::Encoder::new(Vec::new());
    let mut ctx = PROTOCOL_VERSION_12;
    encoder.array(2)?.encode_with(&certificate, &mut ctx)?.encode_with(public, &mut ctx)?;
    Ok((encoder.into_writer(), IssueCounter { next, cold: public }))
}

fn recovery_path(counter: &Path) -> PathBuf {
    let mut path = counter.as_os_str().to_owned();
    path.push(".cert-recovery");
    PathBuf::from(path)
}

/// Make the certificate recoverable independently of its temporary file before replacing the counter.
fn advance_counter(
    certificate: &[u8],
    counter: NamedTempFile,
    counter_path: &Path,
    recovery: &Path,
) -> anyhow::Result<()> {
    prepare_file(recovery, certificate)?
        .persist_noclobber(recovery)
        .with_context(|| format!("could not save recovery certificate at {}; counter unchanged", recovery.display()))?;
    sync_parent(recovery).with_context(|| {
        format!("could not synchronize recovery certificate at {}; counter unchanged", recovery.display())
    })?;
    counter.persist(counter_path).with_context(|| {
        format!(
            "could not advance issue counter {}; signed certificate retained at {}",
            counter_path.display(),
            recovery.display(),
        )
    })?;
    sync_parent(counter_path).with_context(|| {
        format!(
            "could not synchronize issue counter; signed certificate retained at {}. Check the counter before issuing again",
            recovery.display(),
        )
    })
}

fn publish_certificate(file: NamedTempFile, destination: &Path, recovery: &Path) -> anyhow::Result<()> {
    file.persist_noclobber(destination)
        .map_err(|error| anyhow::Error::from(error.error))
        .and_then(|_| sync_parent(destination))
        .with_context(|| format!(
            "counter advanced; signed certificate retained at {}. Recover it to the intended destination instead of issuing again",
            recovery.display(),
        ))?;
    fs::remove_file(recovery)
        .with_context(|| format!("certificate published, but could not remove recovery file {}", recovery.display()))?;
    sync_parent(recovery).context("certificate published, but could not synchronize recovery file removal")
}

/// The lock lives beside the counter, because atomic counter replacement changes its inode.
fn lock_counter(counter: &Path) -> anyhow::Result<File> {
    let mut path = counter.as_os_str().to_owned();
    path.push(".lock");
    let path = PathBuf::from(path);
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(metadata.is_file(), "counter lock must be a regular file: {}", path.display());
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).with_context(|| format!("could not open counter lock {}", path.display()))?;
    file.try_lock().with_context(|| {
        format!("could not lock issue counter at {}; another issuer may be running", path.display())
    })?;
    Ok(file)
}

struct KesVerificationKey(VerificationKey);

impl HasEnvelope for KesVerificationKey {
    const ACCEPTED_TYPES: &'static [&'static str] = &["KesVerificationKey_ed25519_kes_2^6"];
}

impl<'d, C: cbor::HasProtocolVersion> cbor::Decode<'d, C> for KesVerificationKey {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(Self(d.decode_with(ctx)?))
    }
}

fn read_kes_verification_key(path: &Path) -> anyhow::Result<VerificationKey> {
    let mut ctx = PROTOCOL_VERSION_12;
    read_key_file::<32, _, _, ReadEnvelopeError>(path, &mut ctx, |bytes| {
        Ok(KesVerificationKey(VerificationKey::from(*bytes)))
    })
    .map(|key| key.0)
    .with_context(|| format!("could not read KES verification key {}", path.display()))
}
