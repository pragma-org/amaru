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
    path::{Path, PathBuf},
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::cardano::text_envelope::{self, ToTextEnvelope};
use amaru_ouroboros::kes::SecretKey;
use anyhow::{Context, ensure};
use clap::{Args, Subcommand};
use tempfile::NamedTempFile;

#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// Manage hot KES signing keys.
    #[command(subcommand)]
    Kes(KesCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum KesCommand {
    /// Generate a KES key pair.
    Create(CreateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CreateArgs {
    /// Destination for the KES signing key.
    #[arg(long)]
    signing_key: PathBuf,

    /// Destination for the KES verification key. Not generated if unspecified.
    #[arg(long)]
    verification_key: Option<PathBuf>,
}

impl KeysCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Kes(KesCommand::Create(args)) => {
                Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { create(args) })
            }
        }
    }
}

fn create(args: CreateArgs) -> anyhow::Result<()> {
    let CreateArgs { signing_key, verification_key } = args;

    if let Some(verification_key) = verification_key.as_ref() {
        ensure!(&signing_key != verification_key, "KES key paths must differ");
    }

    for path in [Some(&signing_key), verification_key.as_ref()].iter().flatten() {
        let exists = path.try_exists().with_context(|| format!("could not check {}", path.display()))?;
        ensure!(!exists, "{} already exists", path.display());
    }

    let (secret, public) = SecretKey::generate().context("could not generate KES key")?;
    let signing = prepare_key(&secret, &signing_key)?;

    if let Some(verification_key) = verification_key.as_ref() {
        let verification = prepare_key(&public, verification_key)?;
        verification
            .persist_noclobber(verification_key)
            .with_context(|| format!("could not save {}", verification_key.display()))?;
    };

    signing
        .persist_noclobber(&signing_key)
        .inspect_err(|_| {
            if let Some(verification_key) = verification_key.as_ref() {
                let _ = fs::remove_file(verification_key);
            }
        })
        .with_context(|| format!("could not save {}", signing_key.display()))?;
    Ok(())
}

fn prepare_key(key: &impl ToTextEnvelope, path: &Path) -> anyhow::Result<NamedTempFile> {
    let mut file = NamedTempFile::new_in(parent(path))
        .with_context(|| format!("could not create temporary key file for {}", path.display()))?;
    text_envelope::write(key, &mut file).with_context(|| format!("could not write key file for {}", path.display()))?;
    Ok(file)
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{KesEvolution, cbor};
    use amaru_ouroboros::kes::PublicKey;
    use tempfile::TempDir;
    use test_case::test_case;

    use super::*;

    fn args(directory: &Path) -> CreateArgs {
        CreateArgs { signing_key: directory.join("kes.skey"), verification_key: Some(directory.join("kes.vkey")) }
    }

    #[test]
    fn generated_key_files_sign_and_verify() {
        let directory = TempDir::new().unwrap();
        create(args(directory.path())).unwrap();
        let mut secret = SecretKey::from_file(directory.path().join("kes.skey")).unwrap();
        assert_eq!(secret.period(), KesEvolution::from(0));
        let envelope: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join("kes.vkey")).unwrap()).unwrap();
        assert_eq!(envelope["type"], "KesVerificationKey_ed25519_kes_2^6");
        let payload = hex::decode(envelope["cborHex"].as_str().unwrap()).unwrap();
        let mut decoder = cbor::Decoder::new(&payload);
        let key_bytes = cbor::decode_bytes(&mut decoder).unwrap();
        let public = PublicKey::try_from(key_bytes.as_ref()).unwrap();
        assert_eq!(decoder.position(), payload.len());
        let message = b"header body";
        for period in [0, 63] {
            let period = KesEvolution::from(period);
            secret.evolve_to(period).unwrap();
            secret.sign(message).verify(period, &public, message).unwrap();
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::metadata(directory.path().join("kes.skey")).unwrap().permissions();
            assert_eq!(permissions.mode() & 0o777, 0o600);
        }
    }

    #[test_case("kes.skey"; "existing signing key")]
    #[test_case("kes.vkey"; "existing verification key")]
    fn refuses_to_replace_existing_keys(name: &str) {
        let directory = TempDir::new().unwrap();
        let existing = directory.path().join(name);
        fs::write(&existing, b"existing key").unwrap();
        assert!(create(args(directory.path())).is_err());
        assert_eq!(fs::read(existing).unwrap(), b"existing key");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test_case("kes.skey"; "signing key")]
    #[test_case("kes.vkey"; "verification key")]
    fn refuses_to_replace_dangling_symlinks(name: &str) {
        let directory = TempDir::new().unwrap();
        let existing = directory.path().join(name);
        std::os::unix::fs::symlink("missing", &existing).unwrap();
        assert!(create(args(directory.path())).is_err());
        assert_eq!(fs::read_link(existing).unwrap(), Path::new("missing"));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn reports_failed_paths_without_leaving_key_files() {
        let directory = TempDir::new().unwrap();
        let mut args = args(directory.path());
        let missing = directory.path().join("missing").join("kes.skey");
        args.signing_key = missing.clone();
        let error = create(args).unwrap_err();
        assert!(error.to_string().contains(&missing.display().to_string()));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn refuses_identical_paths() {
        let directory = TempDir::new().unwrap();
        let mut args = args(directory.path());
        args.verification_key = Some(args.signing_key.clone());
        assert!(create(args).is_err());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn rolls_back_verification_key_when_signing_key_cannot_be_saved() {
        let directory = TempDir::new().unwrap();
        fs::create_dir(directory.path().join("alias")).unwrap();
        let mut args = args(directory.path());
        args.verification_key = Some(directory.path().join("alias/../kes.skey"));
        assert!(create(args).is_err());
        assert!(!directory.path().join("kes.skey").exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
