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
    collections::BTreeSet,
    io::{self, Write},
    path::PathBuf,
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{Credential, Hasher, NetworkName, PoolId, RewardAccount, ShelleyAddress, StakeReference};
use amaru_ouroboros::credentials::{generate_cold_key_pair, generate_payment_key_pair, generate_stake_key_pair};
use anyhow::{Context, ensure};

use super::{
    cold::{IssueCounter, prepare_counter},
    ensure_absent, prepare_file, publish_new,
};

/// Generate pool cold keys and optional payment and owner/reward stake keys offline.
///
/// Keys are raw bytes; the counter is binary CBOR. Supply both output paths to generate
/// a payment or stake key pair. With both pairs, the payment address contains the stake
/// credential. Without a stake pair, it is an enterprise address with no staking rights.
/// Prints public values as JSON to stdout, omitting keys and addresses that were not generated.
/// Run offline and keep signing keys and the counter there.
/// Existing files are never replaced; destination directories must already exist.
#[derive(Debug, clap::Args)]
pub struct CreateArgs {
    #[command(flatten)]
    network: amaru::args::Network,

    /// Destination for the raw 32-byte cold signing seed.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    cold_signing_key: PathBuf,

    /// Destination for the raw 32-byte cold verification key.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    cold_verification_key: PathBuf,

    /// Destination for the binary CBOR issue counter, starting at zero.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    counter: PathBuf,

    /// Destination for the raw 32-byte payment signing seed.
    #[arg(long, requires = "payment_verification_key", value_name = amaru::value_names::FILEPATH)]
    payment_signing_key: Option<PathBuf>,

    /// Destination for the raw 32-byte payment verification key.
    #[arg(long, requires = "payment_signing_key", value_name = amaru::value_names::FILEPATH)]
    payment_verification_key: Option<PathBuf>,

    /// Destination for the raw 32-byte owner/reward stake signing seed.
    #[arg(long, requires = "stake_verification_key", value_name = amaru::value_names::FILEPATH)]
    stake_signing_key: Option<PathBuf>,

    /// Destination for the raw 32-byte owner/reward stake verification key.
    #[arg(long, requires = "stake_signing_key", value_name = amaru::value_names::FILEPATH)]
    stake_verification_key: Option<PathBuf>,
}

impl CreateArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    pub fn run(self) -> anyhow::Result<()> {
        ensure!(
            self.payment_signing_key.is_some() == self.payment_verification_key.is_some(),
            "payment signing and verification key paths must be supplied together",
        );
        ensure!(
            self.stake_signing_key.is_some() == self.stake_verification_key.is_some(),
            "stake signing and verification key paths must be supplied together",
        );
        let paths = [
            Some(self.cold_signing_key.as_path()),
            Some(self.cold_verification_key.as_path()),
            Some(self.counter.as_path()),
            self.payment_signing_key.as_deref(),
            self.payment_verification_key.as_deref(),
            self.stake_signing_key.as_deref(),
            self.stake_verification_key.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        ensure!(paths.iter().collect::<BTreeSet<_>>().len() == paths.len(), "output paths must differ");
        paths.into_iter().try_for_each(ensure_absent)?;

        let (cold, cold_public) = generate_cold_key_pair().context("could not generate cold key")?;
        let payment_paths = self.payment_signing_key.as_deref().zip(self.payment_verification_key.as_deref());
        let stake_paths = self.stake_signing_key.as_deref().zip(self.stake_verification_key.as_deref());
        let payment =
            payment_paths.map(|_| generate_payment_key_pair()).transpose().context("could not generate payment key")?;
        let stake =
            stake_paths.map(|_| generate_stake_key_pair()).transpose().context("could not generate stake key")?;
        let network = NetworkName::from(self.network);
        let mut public = serde_json::json!({
            "network": network.to_string(),
            "cold_verification_key": hex::encode(cold_public.as_slice()),
            "pool_id": PoolId::from(cold_public).to_string(),
        });
        let stake_credential = stake.as_ref().map(|(_, verification)| {
            let credential = Credential::KeyHash(Hasher::<224>::hash(verification.as_slice()));
            public["stake_verification_key"] = hex::encode(verification.as_slice()).into();
            public["reward_address"] = RewardAccount::new(network.into(), credential).to_bech32().into();
            credential
        });
        if let Some((_, verification)) = &payment {
            let credential = Credential::KeyHash(Hasher::<224>::hash(verification.as_slice()));
            let address =
                ShelleyAddress::new(network.into(), credential, stake_credential.map(StakeReference::Credential));
            public["payment_verification_key"] = hex::encode(verification.as_slice()).into();
            public["payment_address"] = address.to_bech32().into();
        }
        let mut files = vec![
            (self.cold_signing_key.as_path(), prepare_file(&self.cold_signing_key, cold.as_bytes())?),
            (self.cold_verification_key.as_path(), prepare_file(&self.cold_verification_key, cold_public.as_slice())?),
            (self.counter.as_path(), prepare_counter(&IssueCounter { next: 0, cold: cold_public }, &self.counter)?),
        ];
        if let (Some((key, public_key)), Some((signing, verification))) = (&payment, payment_paths) {
            files.push((signing, prepare_file(signing, key.as_bytes())?));
            files.push((verification, prepare_file(verification, public_key.as_slice())?));
        }
        if let (Some((key, public_key)), Some((signing, verification))) = (&stake, stake_paths) {
            files.push((signing, prepare_file(signing, key.as_bytes())?));
            files.push((verification, prepare_file(verification, public_key.as_slice())?));
        }
        publish_new(files)?;
        let mut output = io::stdout().lock();
        serde_json::to_writer_pretty(&mut output, &public).context("keys saved, but could not write public values")?;
        writeln!(output).context("keys saved, but could not finish writing public values")
    }
}
