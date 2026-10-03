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
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_ouroboros::kes::{PublicKey, SecretKey};
use anyhow::{Context, ensure};
use clap::{Args, Subcommand};
use tempfile::NamedTempFile;
use zeroize::Zeroizing;

/// A cardano-cli text envelope for a KES key.
struct TextEnvelope<'a> {
    r#type: &'static str,
    description: &'static str,
    cbor_prefix: &'static str,
    bytes: &'a [u8],
}

impl<'a> TextEnvelope<'a> {
    fn kes_signing(key: &'a SecretKey) -> Self {
        // SAFETY: The CLI needs the secret bytes to write the requested signing-key file.
        // Both the encoded payload and envelope are wiped after use.
        let bytes = unsafe { &key.leak_into_bytes()[..SecretKey::SIZE] };
        Self { r#type: "KesSigningKey_ed25519_kes_2^6", description: "KES Signing Key", cbor_prefix: "590260", bytes }
    }

    fn kes_verification(key: &'a PublicKey) -> Self {
        Self {
            r#type: "KesVerificationKey_ed25519_kes_2^6",
            description: "KES Verification Key",
            cbor_prefix: "5820",
            bytes: key.as_ref(),
        }
    }

    fn encode(&self) -> Zeroizing<String> {
        let mut cbor_hex = Zeroizing::new(hex::encode(self.bytes));
        cbor_hex.insert_str(0, self.cbor_prefix);
        Zeroizing::new(format!(
            r#"{{"type":"{}","description":"{}","cborHex":"{}"}}"#,
            self.r#type,
            self.description,
            cbor_hex.as_str()
        ))
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// Manage KES signing keys.
    #[command(subcommand)]
    Kes(KesCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum KesCommand {
    /// Generate a cardano-cli compatible KES key pair.
    Create(CreateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CreateArgs {
    /// Destination for the KES signing key.
    #[arg(long)]
    signing_key_file: PathBuf,

    /// Destination for the KES verification key.
    #[arg(long)]
    verification_key_file: PathBuf,
}

impl KeysCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Kes(KesCommand::Create(args)) => {
                Runnable::exit_on_signal(RuntimeKind::Io, move || async move { create(args) })
            }
        }
    }
}

fn create(args: CreateArgs) -> anyhow::Result<()> {
    let CreateArgs { signing_key_file, verification_key_file } = args;
    ensure!(signing_key_file != verification_key_file, "KES key paths must differ");
    ensure!(!signing_key_file.exists(), "{} already exists", signing_key_file.display());
    ensure!(!verification_key_file.exists(), "{} already exists", verification_key_file.display());

    let (secret, public) = SecretKey::generate().context("could not generate KES key")?;
    let mut signing = NamedTempFile::new_in(parent(&signing_key_file))?;
    let mut verification = NamedTempFile::new_in(parent(&verification_key_file))?;
    signing.write_all(TextEnvelope::kes_signing(&secret).encode().as_bytes())?;
    verification.write_all(TextEnvelope::kes_verification(&public).encode().as_bytes())?;

    verification.persist_noclobber(&verification_key_file)?;
    signing.persist_noclobber(&signing_key_file).inspect_err(|_| {
        let _ = fs::remove_file(&verification_key_file);
    })?;
    Ok(())
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}
