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
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use amaru::lifecycle::{Runnable, RuntimeKind, ShutdownHandle};
use amaru_kernel::{
    Address, Credential, HasLovelace, Hash, Hasher, MemoizedTransactionOutput, NetworkName, PoolId, PoolMetadata,
    PoolParams, Relay, RewardAccount, Slot, Transaction, TransactionInput, UnitRationalNumber, Value, VecSet,
    VerificationKey, bech32, cbor::to_cbor_with, vrf,
};
use amaru_ledger::store::ReadStore;
use amaru_metrics::Meter;
use amaru_node::{
    LedgerBlockEvent, LedgerObservers, NodeRunning, RegistrationContext, RegistrationError, RegistrationQuery,
    build_registration,
};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};
use anyhow::{Context, Result, anyhow, ensure};
use parking_lot::Mutex;
use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

use super::{ensure_absent, prepare_key, publish_new};
use crate::cmd::temporary_node;

/// Build an unsigned pool registration using a temporarily synchronized node.
///
/// Requires existing bootstrapped stores, exclusively owned by this command. Waits for a
/// tip less than 60 seconds from the clock and for every --tx-in to be unspent. This does
/// not prove equality with all peers' tips. Sign the result offline with `amaru transaction sign`.
/// Use cold and owner/reward stake verification keys from `amaru pool create`, and its payment
/// address for change. Generate VRF keys separately with `amaru pool vrf create`. Keep signing
/// keys and the issue counter offline; registration neither needs nor advances the counter.
#[derive(Debug, clap::Args)]
pub struct RegisterArgs {
    /// Pool settings JSON: pledge and cost in lovelace, margin as {"numerator":1,"denominator":100},
    /// optional relays (Relay JSON) and metadata ({"url":"...","content_hash":"hex"}).
    /// IP relays use named fields, e.g. {"port":3001,"ipv4":"192.0.2.1","ipv6":"2001:db8::1"}.
    /// Omit an absent address family; null is not accepted.
    /// Omit cost to use the protocol minimum at the registration tip.
    #[arg(value_name = "SETTINGS")]
    settings: PathBuf,

    /// Network to synchronize.
    #[command(flatten)]
    network: amaru::args::Network,

    /// Shelley key payment address on the selected network, e.g. payment_address from `amaru pool create`.
    #[arg(long, env = "AMARU_POOL_CHANGE", value_name = "ADDRESS")]
    change: String,

    /// Raw 32-byte cold verification key from `amaru pool create` or `amaru pool cold create`.
    #[arg(long, env = "AMARU_POOL_COLD", value_name = "FILE")]
    cold: PathBuf,

    /// Register missing owner stake credentials and delegate all owners to this pool.
    /// Replaces any existing pool delegation. Defaults to the reward key when no --owner is given.
    /// Reports each owner's previous pool and intended change; does not verify pledge funding.
    #[arg(long)]
    delegate_owners: bool,

    /// Destination for raw unsigned transaction CBOR. Existing files are never replaced.
    #[arg(long, env = "AMARU_POOL_OUT", value_name = "FILE")]
    out: PathBuf,

    /// Raw 32-byte owner stake verification keys, e.g. from `amaru pool create` (repeatable).
    /// Defaults to the reward verification key.
    #[arg(long, env = "AMARU_POOL_OWNER", value_name = "FILE")]
    owner: Vec<PathBuf>,

    /// Upstream peer (repeatable); defaults to the network's embedded peer snapshot.
    #[arg(long, env = amaru::env_vars::PEER, value_name = "ADDRESS")]
    peer: Vec<String>,

    /// Raw 32-byte reward stake verification key, e.g. the stake verification key from `amaru pool create`.
    #[arg(long, env = "AMARU_POOL_REWARD", value_name = "FILE")]
    reward: PathBuf,

    /// Maximum startup and synchronization wait, in seconds. Shutdown runs after this deadline.
    #[arg(long, env = "AMARU_POOL_TIMEOUT", default_value = "600", value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,

    /// Explicit ADA-only funding input, TXID#INDEX (repeatable). Use this instead of --funding-address.
    #[arg(long, env = "AMARU_POOL_TX_IN", required_unless_present = "funding_address", conflicts_with = "funding_address", value_name = "TXID#INDEX", value_parser = parse_input)]
    tx_in: Vec<TransactionInput>,

    /// Discover ADA-only inputs at this Shelley key address on the local ledger.
    #[arg(long, env = "AMARU_POOL_FUNDING_ADDRESS", value_name = "ADDRESS", value_parser = parse_change_address)]
    funding_address: Option<Address>,

    /// Transaction validity after the captured tip, in slots.
    #[arg(long, env = "AMARU_POOL_VALIDITY", default_value = "3600", value_name = "SLOTS", value_parser = clap::value_parser!(u64).range(1..))]
    validity: u64,

    /// Raw 32-byte VRF verification key generated by `amaru pool vrf create`.
    #[arg(long, env = "AMARU_POOL_VRF", value_name = "FILE")]
    vrf: PathBuf,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,
    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    pledge: u64,
    cost: Option<u64>,
    margin: UnitRationalNumber,
    #[serde(default)]
    relays: Vec<Relay>,
    #[serde(default)]
    metadata: Option<PoolMetadata>,
}

impl RegisterArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::soft(RuntimeKind::Node, move |shutdown, meter| self.run(shutdown, meter))
    }

    async fn run(self, shutdown: ShutdownHandle, meter: Meter) -> Result<()> {
        ensure_absent(&self.out)?;
        let destination = prepare_key(|_| Ok(()), &self.out)?;
        let network = NetworkName::from(self.network);
        let registration = self.registration(network)?;
        let ledger = self.db_ledger.into_path_buf(network);
        let chain = self.db_chain.into_path_buf(network);
        let mut node = temporary_node::builder(network, ledger.clone(), chain, self.peer, meter)?;
        let cancel = shutdown.token();
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_secs(self.timeout))
            .context("timeout is too large")?;
        let mut candidates = None;
        if let Some(address) = self.funding_address {
            ensure!(address.network() == network.into(), "funding address belongs to another network");
            let (inputs, observer) = funding_inputs(ledger, address, cancel.clone(), deadline).await?;
            candidates = Some(inputs);
            node = node.observers(observer);
        }
        let running = temporary_node::start(node).await?;
        let result = await_registration(
            registration.build(&running, network, self.validity, candidates),
            running.termination(),
            &cancel,
            deadline,
        )
        .await;
        let report = running.shutdown().await.context("temporary node cleanup failed")?;
        ensure!(report.is_clean(), "temporary node components failed: {:?}", report.unexpected_exits);
        let (context, tx) = result?;
        ensure!(!cancel.is_cancelled(), "pool registration cancelled");
        write_transaction(destination, &self.out, context, tx)
    }

    fn registration(&self, network: NetworkName) -> Result<Registration> {
        let settings: Settings = serde_json::from_slice(&fs::read(&self.settings)?).context("invalid pool settings")?;
        let change = parse_change_address(&self.change)?;
        ensure!(change.network() == network.into(), "change address belongs to another network");
        let inputs: BTreeSet<_> = self.tx_in.iter().copied().collect();
        ensure!(inputs.len() == self.tx_in.len(), "duplicate --tx-in");
        let reward = Hasher::<224>::hash(load_verification_key(&self.reward)?.as_slice());
        let owners: VecSet<_> = self
            .owner
            .iter()
            .map(|path| load_verification_key(path).map(|key| Hasher::<224>::hash(key.as_slice())))
            .chain(self.owner.is_empty().then_some(Ok(reward)))
            .collect::<Result<_>>()?;
        let vrf = vrf::PublicKey::try_from(fs::read(&self.vrf)?.as_slice())
            .context("VRF verification key must contain exactly 32 raw bytes")?;
        let query = RegistrationQuery {
            inputs,
            pool: PoolId::from(load_verification_key(&self.cold)?),
            reward_account: Credential::KeyHash(reward),
            owners: if self.delegate_owners { owners.iter().copied().collect() } else { BTreeSet::new() },
        };
        Ok(Registration {
            query,
            settings,
            owners,
            vrf: Hasher::<256>::hash(vrf.as_ref()),
            change,
            delegate_owners: self.delegate_owners,
        })
    }
}

struct Registration {
    query: RegistrationQuery,
    settings: Settings,
    owners: VecSet<Hash<28>>,
    vrf: Hash<32>,
    change: Address,
    delegate_owners: bool,
}

impl Registration {
    async fn build(
        mut self,
        running: &NodeRunning,
        network: NetworkName,
        validity: u64,
        candidates: Option<FundingInputs>,
    ) -> Result<(RegistrationContext, Transaction)> {
        loop {
            if let Some(candidates) = &candidates {
                self.query.inputs = candidates.take_candidates();
            }
            let mut context = running.registration_context(self.query.clone()).await?;
            if let Some(candidates) = &candidates {
                candidates.retain_eligible(&mut context.inputs);
            }
            if temporary_node::live(network, context.point)? {
                let expires = context.point.slot().as_u64().checked_add(validity).context("validity overflow")?;
                if candidates.is_some() {
                    if let Some(tx) = self.select_transaction(&context, Slot::from(expires))? {
                        return Ok((context, tx));
                    }
                } else if self.query.inputs.iter().all(|input| context.inputs.contains_key(input)) {
                    let tx = self.transaction(&context, Slot::from(expires))?;
                    return Ok((context, tx));
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    fn select_transaction(&self, context: &RegistrationContext, expires: Slot) -> Result<Option<Transaction>> {
        let mut candidates: Vec<_> = context.inputs.iter().collect();
        candidates.sort_by_key(|(input, output)| (std::cmp::Reverse(output.lovelace()), **input));
        let pool = self.pool(context);
        let mut inputs = BTreeSet::new();
        for (input, _) in candidates {
            inputs.insert(*input);
            match build_registration(context, &inputs, &pool, &self.change, expires, self.delegate_owners) {
                Err(RegistrationError::InsufficientFunds) => continue,
                result => return Ok(Some(result?)),
            }
        }
        Ok(None)
    }

    fn transaction(&self, context: &RegistrationContext, expires: Slot) -> Result<Transaction> {
        Ok(build_registration(
            context,
            &self.query.inputs,
            &self.pool(context),
            &self.change,
            expires,
            self.delegate_owners,
        )?)
    }

    fn pool(&self, context: &RegistrationContext) -> PoolParams {
        PoolParams {
            id: self.query.pool,
            vrf: self.vrf,
            pledge: self.settings.pledge,
            cost: self.settings.cost.unwrap_or(context.parameters.min_pool_cost),
            margin: self.settings.margin,
            reward_account: RewardAccount::new(self.change.network(), self.query.reward_account),
            owners: self.owners.clone(),
            relays: self.settings.relays.clone(),
            metadata: self.settings.metadata.clone(),
        }
    }
}

struct FundingInputs {
    address: Address,
    candidates: Arc<Mutex<BTreeSet<TransactionInput>>>,
}

impl FundingInputs {
    fn observer(&self) -> LedgerObservers {
        let observed = Arc::clone(&self.candidates);
        let address = self.address.clone();
        LedgerObservers::new().on_block(move |event| match event {
            LedgerBlockEvent::Adopted(block) => observed.lock().extend(
                block
                    .utxo
                    .produced
                    .iter()
                    .filter_map(|(input, output)| eligible_funding(&address, output).then_some(*input)),
            ),
            // Undos carry consumed references. Resolve their outputs at the current ledger tip.
            LedgerBlockEvent::Undone(block) => observed.lock().extend(block.utxo.consumed.iter().copied()),
        })
    }

    fn take_candidates(&self) -> BTreeSet<TransactionInput> {
        std::mem::take(&mut *self.candidates.lock())
    }

    fn retain_eligible(&self, inputs: &mut BTreeMap<TransactionInput, MemoizedTransactionOutput>) {
        inputs.retain(|_, output| eligible_funding(&self.address, output));
        // Merge with observer updates that arrived while the query was in flight.
        self.candidates.lock().extend(inputs.keys().copied());
    }
}

fn eligible_funding(address: &Address, output: &MemoizedTransactionOutput) -> bool {
    &output.address == address && matches!(output.value, Value::Coin(_)) && output.script.is_none()
}

async fn funding_inputs(
    ledger: PathBuf,
    address: Address,
    cancel: CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<(FundingInputs, LedgerObservers)> {
    let scan_address = address.clone();
    let inputs = tokio::task::spawn_blocking(move || -> Result<_> {
        let db = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger))?;
        let mut inputs = BTreeSet::new();
        for (input, output) in db.iter_utxos()? {
            ensure!(!cancel.is_cancelled(), "pool registration cancelled");
            ensure!(tokio::time::Instant::now() < deadline, "funding address scan timed out");
            if eligible_funding(&scan_address, &output) {
                inputs.insert(input);
            }
        }
        Ok(inputs)
    })
    .await??;
    let inputs = FundingInputs { address, candidates: Arc::new(Mutex::new(inputs)) };
    let observer = inputs.observer();
    Ok((inputs, observer))
}

#[expect(clippy::print_stdout)]
fn write_transaction(
    mut destination: NamedTempFile,
    out: &Path,
    mut context: RegistrationContext,
    tx: Transaction,
) -> Result<()> {
    destination.write_all(&to_cbor_with(&tx, &mut context.parameters.protocol_version))?;
    destination.as_file().sync_all()?;
    publish_new([(out, destination)])?;
    let mut report =
        serde_json::json!({"point": context.point, "transaction_id": tx.tx_id(), "fee": tx.body.fee, "out": out});
    if !context.owners.is_empty() {
        let owners = context
            .owners
            .iter()
            .map(|(owner, delegation)| {
                serde_json::json!({
                    "stake_address": RewardAccount::new(context.network, Credential::KeyHash(*owner)).to_bech32(),
                    "previous_pool": delegation.previous_pool.map(|pool| pool.to_string()),
                    "target_pool": context.pool.to_string(),
                    "registration_deposit": delegation.registration_deposit,
                    "action": delegation.action,
                })
            })
            .collect::<Vec<_>>();
        report["owner_delegations"] = owners.into();
    }
    println!("{report}");
    Ok(())
}

async fn await_registration<T>(
    work: impl std::future::Future<Output = Result<T>>,
    termination: impl std::future::Future<Output = ()>,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<T> {
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow!("pool registration cancelled")),
        _ = tokio::time::sleep_until(deadline) => Err(anyhow!("timed out waiting for a live tip and all funding inputs")),
        _ = termination => Err(anyhow!("temporary node stopped before registration was built")),
        result = work => result,
    }?;
    ensure!(!cancel.is_cancelled(), "pool registration cancelled");
    ensure!(tokio::time::Instant::now() < deadline, "pool registration timed out");
    Ok(result)
}

fn parse_change_address(value: &str) -> Result<Address> {
    bech32::decode(value)
        .ok()
        .filter(|(hrp, _)| *hrp == *bech32::HRP_ADDR || *hrp == *bech32::HRP_ADDR_TEST)
        .and_then(|(_, bytes)| Address::from_bytes(&bytes))
        .filter(|address| matches!(address, Address::Shelley(address) if !address.payment().is_script()))
        .context("only Shelley key change addresses in bech32 are supported")
}

fn parse_input(value: &str) -> Result<TransactionInput, String> {
    let (hash, index) = value.split_once('#').ok_or("expected TXID#INDEX")?;
    Ok(TransactionInput {
        transaction_id: hash.parse::<Hash<32>>().map_err(|error| error.to_string())?,
        index: index.parse::<u16>().map_err(|error| error.to_string())?,
    })
}

fn load_verification_key(path: &Path) -> Result<VerificationKey> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let key = amaru_kernel::ed25519::VerifyingKey::try_from(bytes.as_slice())
        .with_context(|| format!("{} must contain a raw 32-byte Ed25519 verification key", path.display()))?;
    Ok(key.to_bytes().into())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: RegisterArgs,
    }

    #[test]
    fn created_pool_keys_build_and_sign_registration_without_advancing_counter() {
        use amaru::exit::SignalState;
        use amaru_kernel::{
            MemoizedDatum, MemoizedTransactionOutput, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, ShelleyAddress,
            StakeReference, Value, cbor::from_cbor_no_leftovers_with,
        };

        use super::super::{PoolCommand, cold::IssueCounter};
        use crate::cli::{Cli as AmaruCli, Command};

        let directory = tempfile::tempdir().unwrap();
        let file = |name: &str| directory.path().join(name);
        let command = |args: &[&str], files: &[(&str, &str)]| {
            let paths = files.iter().flat_map(|(flag, name)| [(*flag).into(), file(name).into_os_string()]);
            AmaruCli::parse_from(args.iter().map(std::ffi::OsString::from).chain(paths)).command
        };
        let signals = SignalState::new_disconnected();
        let run = |command: Command| command.into_runnable().run(&signals).unwrap();
        run(command(
            &["amaru", "pool", "create", "--network", "preprod"],
            &[
                ("--cold-signing-key", "cold.sk"),
                ("--cold-verification-key", "cold.vk"),
                ("--counter", "cold.counter"),
                ("--payment-signing-key", "payment.sk"),
                ("--payment-verification-key", "payment.vk"),
                ("--stake-signing-key", "stake.sk"),
                ("--stake-verification-key", "stake.vk"),
            ],
        ));
        run(command(
            &["amaru", "pool", "vrf", "create"],
            &[("--signing-key", "vrf.sk"), ("--verification-key", "vrf.vk")],
        ));

        let network = NetworkName::Preprod;
        let stake = Hasher::<224>::hash(load_verification_key(&file("stake.vk")).unwrap().as_slice());
        let payment = Hasher::<224>::hash(load_verification_key(&file("payment.vk")).unwrap().as_slice());
        let change = ShelleyAddress::new(
            network.into(),
            Credential::KeyHash(payment),
            Some(StakeReference::Credential(Credential::KeyHash(stake))),
        );
        let counter_before = fs::read(file("cold.counter")).unwrap();
        let mut version = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.protocol_version;
        let counter: IssueCounter = from_cbor_no_leftovers_with(&counter_before, &mut version).unwrap();
        assert_eq!(counter.next, 0);
        fs::write(file("pool.json"), r#"{"pledge":0,"margin":{"numerator":1,"denominator":100}}"#).unwrap();
        let input = TransactionInput { transaction_id: Hash::new([0; 32]), index: 0 };
        let change_address = change.to_bech32();
        let Command::Pool(PoolCommand::Register(args)) = command(
            &[
                "amaru",
                "pool",
                "register",
                file("pool.json").to_str().unwrap(),
                "--network",
                "preprod",
                "--change",
                &change_address,
                "--tx-in",
                &input.to_string(),
            ],
            &[("--cold", "cold.vk"), ("--vrf", "vrf.vk"), ("--reward", "stake.vk"), ("--out", "registration.cbor")],
        ) else {
            panic!("expected pool register")
        };
        let registration = args.registration(network).unwrap();
        assert_eq!(registration.query.pool, PoolId::from(counter.cold));
        assert_eq!(registration.owners, [stake].into_iter().collect());
        assert_eq!(registration.query.reward_account, Credential::KeyHash(stake));
        assert_eq!(registration.change, Address::Shelley(change));
        let output = MemoizedTransactionOutput::new(
            registration.change.clone(),
            Value::Coin(1_000_000_000),
            MemoizedDatum::None,
            None,
        );
        let context = RegistrationContext {
            point: Point::Origin,
            network: network.into(),
            parameters: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            inputs: [(input, output)].into(),
            pool: registration.query.pool,
            pool_registered: false,
            reward_account: registration.query.reward_account,
            reward_registered: false,
            owners: Default::default(),
        };
        let tx = registration.transaction(&context, Slot::from(3600)).unwrap();
        let id = tx.tx_id();
        write_transaction(prepare_key(|_| Ok(()), &args.out).unwrap(), &args.out, context, tx).unwrap();
        run(command(
            &["amaru", "transaction", "sign"],
            &[
                ("--transaction", "registration.cbor"),
                ("--signing-key", "payment.sk"),
                ("--signing-key", "cold.sk"),
                ("--signing-key", "stake.sk"),
                ("--output", "signed.cbor"),
            ],
        ));
        let signed: Transaction =
            from_cbor_no_leftovers_with(&fs::read(file("signed.cbor")).unwrap(), &mut version).unwrap();
        assert_eq!(signed.tx_id(), id);
        let witnesses = signed.witnesses.verification_key_witness.as_ref().unwrap();
        assert_eq!(witnesses.len(), 3);
        for witness in witnesses.iter() {
            let key = amaru_kernel::ed25519::VerifyingKey::try_from(witness.verification_key.as_slice()).unwrap();
            let signature = amaru_kernel::ed25519::Signature::from_slice(witness.signature.as_slice()).unwrap();
            key.verify_strict(id.as_ref(), &signature).unwrap();
        }
        assert_eq!(fs::read(file("cold.counter")).unwrap(), counter_before);
    }

    #[tokio::test]
    async fn cancellation_timeout_and_node_failure_take_priority_over_ready_output() {
        use std::future::{pending, ready};

        use tokio_util::sync::CancellationToken;

        let cancel = CancellationToken::new();
        cancel.cancel();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let error = await_registration(ready(Ok(())), pending(), &cancel, deadline).await.unwrap_err();
        assert!(error.to_string().contains("cancelled"));

        let cancel = CancellationToken::new();
        let error =
            await_registration(ready(Ok(())), pending(), &cancel, tokio::time::Instant::now()).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));

        let error = await_registration(ready(Ok(())), ready(()), &cancel, deadline).await.unwrap_err();
        assert!(error.to_string().contains("node stopped"));
        await_registration(ready(Ok(())), pending(), &cancel, deadline).await.unwrap();
    }

    #[test]
    fn parses_registration_and_rejects_invalid_timeout_and_input() {
        let args = "register pool.json --network preprod --cold cold.vk --vrf vrf.vk --reward stake.vk \
            --change addr_test1... --out registration.cbor --tx-in \
            0000000000000000000000000000000000000000000000000000000000000000#1"
            .split_whitespace()
            .collect::<Vec<_>>();
        let parsed = Cli::try_parse_from(&args).unwrap().args;
        assert_eq!(parsed.timeout, 600);
        assert_eq!(parsed.validity, 3600);
        assert_eq!(parsed.tx_in[0].index, 1);
        assert!(Cli::try_parse_from(args.into_iter().chain(["--timeout", "0"])).is_err());
        for value in ["bad", "00#0", "0000000000000000000000000000000000000000000000000000000000000000#65536"] {
            assert!(parse_input(value).is_err());
        }
    }

    #[test]
    fn change_accepts_only_shelley_key_addresses() {
        use amaru_kernel::{Network, ShelleyAddress};

        let key = Credential::KeyHash(Hash::new([1; 28]));
        let address = Address::Shelley(ShelleyAddress::new(Network::Testnet, key, None));
        assert_eq!(parse_change_address(&address.to_string()).unwrap(), address);
        let script =
            Address::Shelley(ShelleyAddress::new(Network::Testnet, Credential::ScriptHash(Hash::new([2; 28])), None));
        for unsupported in [
            script.to_string(),
            address.to_hex(),
            RewardAccount::new(Network::Testnet, key).to_string(),
            bech32::encode(*bech32::HRP_STAKE_TEST, address.to_vec()).unwrap(),
            "2cWKMJemoBaiqkR9D1YZ2xQ2BhVxzauukrsxm8ttZUrto1f7kr5J1tD9uhtEtTc9U4PuF".into(),
        ] {
            assert!(parse_change_address(&unsupported).is_err());
        }
    }

    #[test]
    fn settings_validate_margin_and_reject_unknown_fields() {
        let settings: Settings =
            serde_json::from_str(r#"{"pledge":100000000,"cost":170000000,"margin":{"numerator":1,"denominator":100}}"#)
                .unwrap();
        assert_eq!(settings.margin, UnitRationalNumber::new(1, 100).unwrap());
        assert!(settings.relays.is_empty());
        for json in [
            r#"{"pledge":0,"cost":0,"margin":{"numerator":2,"denominator":1}}"#,
            r#"{"pledge":0,"cost":0,"margin":{"numerator":1,"denominator":0}}"#,
            r#"{"pledge":0,"cost":0,"margin":{"numerator":0,"denominator":1},"typo":1}"#,
        ] {
            assert!(serde_json::from_str::<Settings>(json).is_err());
        }
    }

    #[test]
    fn reads_raw_public_keys_and_rejects_other_formats() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("key.vk");
        let key = amaru_kernel::ed25519::SigningKey::from_bytes(&[1; 32]).verifying_key();
        fs::write(&path, key.as_bytes()).unwrap();
        assert_eq!(load_verification_key(&path).unwrap().as_slice(), key.as_bytes());
        for unsupported in [
            vec![0; 31],
            vec![0; 33],
            vec![0; 64],
            hex::encode(key.as_bytes()).into_bytes(),
            serde_json::json!({"type": "StakeVerificationKeyShelley_ed25519", "cborHex": "5820"})
                .to_string()
                .into_bytes(),
        ] {
            fs::write(&path, unsupported).unwrap();
            assert!(load_verification_key(&path).is_err());
        }
        let mut invalid = [0; 32];
        invalid[0] = 2;
        fs::write(&path, invalid).unwrap();
        assert!(load_verification_key(&path).is_err());
    }
}
