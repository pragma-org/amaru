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
    io::{self, Write},
    path::{Path, PathBuf},
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{Hasher, hash::size::POOL_COLD_KEY};
use amaru_ouroboros::{kes::SecretKey, load_cold_verification_key, load_operational_certificate};
use anyhow::{Context, ensure};
use clap::{Args, Subcommand};
use serde::Serialize;
use tempfile::NamedTempFile;

#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// Manage hot KES signing keys.
    #[command(subcommand)]
    Kes(KesCommand),

    /// Inspect and verify operational certificates.
    #[command(subcommand)]
    Opcert(OpcertCommand),

    /// Derive the pool ID from a cold verification key.
    PoolId(PoolIdArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum KesCommand {
    /// Generate a KES key pair.
    Create(CreateArgs),

    /// Show the current KES period and how many periods remain before key rotation is required.
    Period(PeriodArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum OpcertCommand {
    /// Inspect an operational certificate and display its fields.
    Inspect(InspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CreateArgs {
    /// Destination for the raw binary KES signing key.
    #[arg(long)]
    signing_key: PathBuf,

    /// Destination for the raw binary KES verification key. Not generated if unspecified.
    #[arg(long)]
    verification_key: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PeriodArgs {
    /// Operational certificate used to determine the KES period at which the key was issued.
    #[arg(long, value_name = amaru::value_names::FILEPATH, env = amaru::env_vars::OPERATOR_OPERATIONAL_CERTIFICATE)]
    opcert: PathBuf,

    /// Current slot number.
    #[arg(long, value_name = amaru::value_names::SLOT)]
    slot: u64,

    /// Number of slots per KES period.
    ///
    /// Defaults to the Cardano mainnet value of 129600.
    #[arg(long, value_name = amaru::value_names::UINT, default_value_t = 129_600)]
    slots_per_kes_period: u64,

    /// Maximum number of KES evolutions before key rotation is required.
    ///
    /// Defaults to the Cardano mainnet value of 62.
    #[arg(long, value_name = amaru::value_names::UINT, default_value_t = 62)]
    max_kes_evolutions: u64,
}

#[derive(Debug, Args)]
pub(crate) struct InspectArgs {
    /// Operational certificate to inspect.
    #[arg(value_name = amaru::value_names::FILEPATH)]
    opcert: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct PoolIdArgs {
    /// Cold verification key file (`StakePoolVerificationKey_ed25519` text envelope).
    #[arg(value_name = amaru::value_names::FILEPATH)]
    cold_vkey: PathBuf,
}

impl KeysCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Kes(KesCommand::Create(args)) => {
                Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { create(args) })
            }
            Self::Kes(KesCommand::Period(args)) => {
                Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { kes_period(args) })
            }
            Self::Opcert(OpcertCommand::Inspect(args)) => {
                Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { opcert_inspect(args) })
            }
            Self::PoolId(args) => Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { pool_id(args) }),
        }
    }
}

#[derive(Serialize)]
struct OpcertInfo {
    pool_id: String,
    sequence_number: u64,
    kes_period: u64,
    hot_verification_key: String,
}

#[derive(Serialize)]
struct KesPeriodInfo {
    current_kes_period: u64,
    opcert_kes_period: u64,
    periods_used: u64,
    periods_remaining: u64,
}

#[expect(clippy::print_stdout)]
fn opcert_inspect(args: InspectArgs) -> anyhow::Result<()> {
    let (cert, cold) = load_operational_certificate(&args.opcert)
        .with_context(|| format!("failed to load operational certificate: {}", args.opcert.display()))?;

    let pool_id = Hasher::<{ 8 * POOL_COLD_KEY }>::hash(cold.as_slice());

    let info = OpcertInfo {
        pool_id: hex::encode(pool_id.as_ref()),
        sequence_number: cert.operational_cert_sequence_number,
        kes_period: u64::from(cert.operational_cert_kes_period),
        hot_verification_key: hex::encode(cert.operational_cert_hot_verification_key.as_slice()),
    };

    println!("{}", serde_json::to_string_pretty(&info)?);
    Ok(())
}

#[expect(clippy::print_stdout)]
fn kes_period(args: PeriodArgs) -> anyhow::Result<()> {
    let (cert, _) = load_operational_certificate(&args.opcert)
        .with_context(|| format!("failed to load operational certificate: {}", args.opcert.display()))?;

    let opcert_kes_period = u64::from(cert.operational_cert_kes_period);
    let current_kes_period = args.slot / args.slots_per_kes_period;
    let periods_used = current_kes_period.saturating_sub(opcert_kes_period);
    let periods_remaining = args.max_kes_evolutions.saturating_sub(periods_used);

    let info = KesPeriodInfo { current_kes_period, opcert_kes_period, periods_used, periods_remaining };

    println!("{}", serde_json::to_string_pretty(&info)?);
    Ok(())
}

#[expect(clippy::print_stdout)]
fn pool_id(args: PoolIdArgs) -> anyhow::Result<()> {
    let cold = load_cold_verification_key(&args.cold_vkey)
        .with_context(|| format!("failed to load cold verification key: {}", args.cold_vkey.display()))?;

    let pool_id = Hasher::<{ 8 * POOL_COLD_KEY }>::hash(cold.as_slice());
    println!("{}", hex::encode(pool_id.as_ref()));
    Ok(())
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
    let signing = prepare_key(|file| secret.write_raw(file), &signing_key)?;

    if let Some(verification_key) = verification_key.as_ref() {
        let verification = prepare_key(|file| file.write_all(public.as_ref()), verification_key)?;
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

fn prepare_key(
    write_raw: impl FnOnce(&mut NamedTempFile) -> io::Result<()>,
    path: &Path,
) -> anyhow::Result<NamedTempFile> {
    let mut file = NamedTempFile::new_in(parent(path))
        .with_context(|| format!("could not create temporary key file for {}", path.display()))?;
    write_raw(&mut file).with_context(|| format!("could not write key file for {}", path.display()))?;
    Ok(file)
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

#[cfg(test)]
mod tests {
    use amaru_kernel::KesEvolution;
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
        let verification = fs::read(directory.path().join("kes.vkey")).unwrap();
        let public = PublicKey::try_from(verification.as_slice()).unwrap();
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
