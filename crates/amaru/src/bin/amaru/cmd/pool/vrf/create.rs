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

use std::{io::Write, path::PathBuf};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_ouroboros::vrf;
use anyhow::Context;

use super::super::write_key_pair;

/// Generate a VRF key pair.
///
/// Signing keys are unencrypted. Use directories only you can modify, with access restricted
/// to your account.
#[derive(Debug, clap::Args)]
pub struct CreateArgs {
    /// Destination for the raw binary VRF signing key. Must not already exist.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    signing_key: PathBuf,

    /// Destination for the raw binary VRF verification key. Must not already exist.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    verification_key: PathBuf,
}

impl CreateArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    pub fn run(self) -> anyhow::Result<()> {
        let Self { signing_key, verification_key } = self;
        let (secret, public) = vrf::SecretKey::generate().context("could not generate VRF key")?;
        write_key_pair(
            &signing_key,
            Some(&verification_key),
            |file| secret.write_raw(file),
            |file| file.write_all(public.as_ref()),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use amaru_ouroboros::vrf::{Input, PublicKey, SecretKey};
    use tempfile::TempDir;

    #[test]
    fn generated_key_files_prove_and_verify() {
        let directory = TempDir::new().unwrap();
        let signing_key = directory.path().join("vrf.skey");
        let verification_key = directory.path().join("vrf.vkey");
        super::CreateArgs { signing_key: signing_key.clone(), verification_key: verification_key.clone() }
            .run()
            .unwrap();
        assert_eq!(fs::metadata(&signing_key).unwrap().len(), SecretKey::SIZE as u64);
        let secret = SecretKey::from_file(signing_key).unwrap();
        let verification = fs::read(verification_key).unwrap();
        let public = PublicKey::try_from(verification.as_slice()).unwrap();
        assert_eq!(public, PublicKey::from(&secret));
        let input = Input::from(&[11u8; Input::SIZE]);
        secret.prove(&input).verify(&public, &input).unwrap();
    }
}
