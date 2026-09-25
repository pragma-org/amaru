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

//! Minted-chain world tests.
//!
//! Five pools share one synthesized stake distribution and produce the chain.
//! Validation and forging stay on the production path. See EDR-011 "World tests".

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use amaru_consensus::stages::forge_block::TestCredentials;
use amaru_kernel::{
    Anchor, Block, BodyParts, CertificatePointer, Constitution, ConstitutionalCommitteeStatus, Credential, Epoch,
    EraBound, EraHistory, EraName, EraParams, EraSummary, GlobalParameters, Hash, Hasher, Header, HeaderBody, IsHeader,
    KesPeriod, MaxString128, Network, NetworkName, Nonce, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_GLOBAL_PARAMETERS, Peer, Point, PoolParams, Pots, ProposalsRoots, ProtocolParameters, RationalNumber,
    RewardAccount, Slot, TransactionPointer, VrfCert, cardano::network_block::NetworkBlock, ed25519,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::initial_stake_distributions,
    store::{
        Columns, Store, TransactionalContext,
        columns::{accounts, pools},
    },
};
use amaru_ouroboros::{BaseReadChainStore, ChainStore, WriteChainStore};
use amaru_ouroboros_traits::{ForgingCredentials, Nonces, PoolSummaries, PoolSummary};
use amaru_protocols::store_effects::ResourceHeaderStore;
use amaru_pure_stage::simulation::SimulationRunning;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig, consensus::RocksDBStore};
use tokio::runtime::{Handle, Runtime};
use tracing::{Level, field::Visit};
use tracing_subscriber::{Layer, layer::SubscriberExt, registry};

use super::{
    WorldLoop, build_world_node,
    support::{derive_seed, draw_test_seed},
    world_connection_provider::WorldConnectionProvider,
};

const POOLS: usize = 5;
const F_INV: u64 = 20;
const ACTIVE_SLOT_COEFF_INVERSE: f64 = F_INV as f64;
/// Lovelace delegated to each pool. Large next to one epoch of rewards.
const POOL_STAKE: u64 = 1_000_000_000_000_000;
const ACTIVE_NONCE: [u8; 32] = [9u8; 32];
const SMOKE_BLOCKS: u64 = 160;
const LONG_BLOCKS: u64 = 1_000;
/// Epochs minted after the anchor in the short-epoch run.
const SHORT_EPOCHS: u64 = 4;

/// Epoch geometry shared by the chain store, the ledger, and consensus.
struct MintGeometry {
    security_param: u64,
    epoch_scale: u64,
    slots_per_kes_period: u64,
}

impl MintGeometry {
    /// Epoch of 50_000 slots, nonce freeze of 10_000, ledger stability of 7_500.
    fn standard() -> Self {
        Self {
            security_param: 125,
            epoch_scale: 20,
            slots_per_kes_period: PREPROD_GLOBAL_PARAMETERS.slots_per_kes_period,
        }
    }

    /// `k = 25` and `scale = 5`, with `f⁻¹` left at 20. Epoch is 2_500 slots and the nonce
    /// freeze is 2_000, which still fits inside the epoch. The KES period sits inside a run
    /// of a few of these epochs.
    fn short_epochs() -> Self {
        Self { security_param: 25, epoch_scale: 5, slots_per_kes_period: 5_000 }
    }

    fn epoch_length(&self) -> u64 {
        F_INV * self.epoch_scale * self.security_param
    }

    /// First slot of epoch 4. That is simulated time 0.
    fn chain_start_slot(&self) -> u64 {
        4 * self.epoch_length()
    }
}

struct MintNetwork {
    global_parameters: GlobalParameters,
    era_history: EraHistory,
    protocol_parameters: ProtocolParameters,
}

struct MintFixture {
    _root: tempfile::TempDir,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    pools: Vec<Arc<TestCredentials>>,
    anchor: Header,
}

#[derive(Clone)]
struct LogRecord {
    target: String,
    name: String,
    fields: BTreeMap<String, String>,
}

struct Capture {
    records: Arc<Mutex<Vec<LogRecord>>>,
}

struct FieldVisitor(BTreeMap<String, String>);

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = FieldVisitor(BTreeMap::new());
        event.record(&mut visitor);
        self.records.lock().expect("log capture").push(LogRecord {
            target: event.metadata().target().to_string(),
            name: event.metadata().name().to_string(),
            fields: visitor.0,
        });
    }
}

fn mint_network(geometry: MintGeometry) -> MintNetwork {
    let global_parameters = GlobalParameters {
        system_start: PREPROD_GLOBAL_PARAMETERS.system_start,
        consensus_security_param: geometry.security_param,
        epoch_length_scale_factor: geometry.epoch_scale,
        active_slot_coeff_inverse: F_INV,
        max_lovelace_supply: PREPROD_GLOBAL_PARAMETERS.max_lovelace_supply,
        slots_per_kes_period: geometry.slots_per_kes_period,
        max_kes_evolution: PREPROD_GLOBAL_PARAMETERS.max_kes_evolution,
    };
    let era = EraSummary {
        start: EraBound { time: Duration::ZERO, slot: Slot::from(0), epoch: Epoch::from(0) },
        end: None,
        params: EraParams::new(global_parameters.epoch_length(), Duration::from_secs(1), EraName::Conway)
            .expect("epoch length"),
    };
    let era_history = EraHistory::new(&[era], global_parameters.stability_window());
    MintNetwork { global_parameters, era_history, protocol_parameters: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone() }
}

fn pool_credentials(index: usize, kes_period: KesPeriod, max_evolutions: u64) -> TestCredentials {
    let cold = ed25519::SigningKey::from_bytes(&[u8::try_from(index + 1).unwrap(); 32]);
    let vrf = [u8::try_from(index + 16).unwrap(); 32];
    TestCredentials::with_vrf_seed(cold, vrf, kes_period, max_evolutions)
}

fn header_for(
    credentials: &TestCredentials,
    slot: Slot,
    block_number: u64,
    parent: Option<Hash<32>>,
    protocol_version: amaru_kernel::ProtocolVersion,
    slots_per_kes_period: u64,
) -> Header {
    let (body_hash, body_size) = Block::body_commitment(&[]).expect("empty body");
    let kes_period = KesPeriod::from(u64::from(slot) / slots_per_kes_period);
    let body = HeaderBody {
        block_number,
        slot: u64::from(slot),
        prev_hash: parent,
        issuer_verification_key: credentials.issuer_verification_key(),
        vrf_verification_key: credentials.vrf_verification_key(),
        vrf_result: VrfCert {
            output: amaru_kernel::Bytes::default(),
            proof: amaru_kernel::cardano::fixed_bytes::FixedBytes::<80>::zeroes(),
        },
        block_body_size: body_size,
        block_body_hash: body_hash,
        operational_cert: credentials.operational_cert(),
        protocol_version,
    };
    let signature = credentials.sign(kes_period, &body).expect("sign anchor");
    Header::new(body, signature)
}

fn store_block(chain: &RocksDBStore, header: &Header, era: &EraHistory) {
    let bytes = BodyParts::from_transactions(std::iter::empty()).expect("parts").encode_block(header);
    let raw = NetworkBlock::from_encoded_block(era, header.slot(), bytes).expect("era").raw_block();
    chain.store_block(&header.hash(), &raw).expect("store block");
}

fn synthesize(network: &MintNetwork) -> MintFixture {
    let root = tempfile::tempdir().expect("tempdir");
    let ledger_dir = root.path().join("ledger");
    let chain_dir = root.path().join("chain");
    let kes_period = KesPeriod::from(0);
    let max_evolutions = u64::from(network.global_parameters.max_kes_evolution);
    let pools: Vec<_> = (0..POOLS).map(|index| Arc::new(pool_credentials(index, kes_period, max_evolutions))).collect();
    let anchor_slot = 4 * network.global_parameters.epoch_length() - 1;

    let parent = header_for(
        pools[0].as_ref(),
        Slot::from(anchor_slot - 1),
        0,
        None,
        network.protocol_parameters.protocol_version,
        network.global_parameters.slots_per_kes_period,
    );
    let anchor = header_for(
        pools[0].as_ref(),
        Slot::from(anchor_slot),
        1,
        Some(parent.hash()),
        network.protocol_parameters.protocol_version,
        network.global_parameters.slots_per_kes_period,
    );

    write_ledger(network, &ledger_dir, &pools, &anchor);
    write_chain(network, &chain_dir, &parent, &anchor);

    MintFixture { _root: root, ledger_dir, chain_dir, pools, anchor }
}

fn write_ledger(network: &MintNetwork, dir: &Path, pools: &[Arc<TestCredentials>], anchor: &Header) {
    let db = RocksDB::empty(&RocksDbConfig::new(dir.to_path_buf())).expect("ledger");
    let tx = db.create_transaction();
    tx.set_protocol_parameters(&network.protocol_parameters).expect("protocol parameters");
    tx.set_constitution(&Constitution {
        anchor: Anchor {
            url: MaxString128::from_str("https://example.com").expect("url"),
            content_hash: Hash::new([0u8; 32]),
        },
        guardrail_script: None,
    })
    .expect("constitution");
    tx.set_governance_activity(GovernanceActivity { consecutive_dormant_epochs: 0 }).expect("governance");
    tx.set_proposals_roots(&ProposalsRoots::default()).expect("proposals roots");
    tx.update_constitutional_committee(
        &ConstitutionalCommitteeStatus::NoConfidence,
        &BTreeMap::new(),
        &BTreeSet::new(),
    )
    .expect("committee");
    tx.with_pots(|mut row| {
        *row.borrow_mut() =
            Pots { treasury: 0, reserves: network.global_parameters.max_lovelace_supply / 2, fees: 0, donations: 0 };
    })
    .expect("pots");
    tx.commit().expect("seed commit");

    let pointer = CertificatePointer {
        transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
        certificate_index: 0,
    };
    let deposit = network.protocol_parameters.stake_pool_deposit;
    let pool_rows: Vec<pools::Value> = pools
        .iter()
        .map(|credentials| {
            let id = credentials.pool_id();
            let vrf = Hasher::<256>::hash(credentials.vrf_verification_key().as_slice());
            let reward = RewardAccount::new(Network::Testnet, Credential::KeyHash(Hash::new([0u8; 28])));
            (
                PoolParams {
                    id,
                    vrf,
                    pledge: 0,
                    cost: 0,
                    margin: RationalNumber { numerator: 0, denominator: 1 },
                    reward_account: reward,
                    owners: Vec::new(),
                    relays: Vec::new(),
                    metadata: None,
                },
                pointer,
                deposit,
            )
        })
        .collect();
    let account_rows: Vec<(accounts::Key, accounts::Value)> = pools
        .iter()
        .enumerate()
        .map(|(index, credentials)| {
            let credential = Credential::KeyHash(Hash::new([u8::try_from(index + 1).unwrap(); 28]));
            (
                credential,
                accounts::Value::Create {
                    pool: amaru_ledger::state::volatile::Resettable::Set((credentials.pool_id(), pointer)),
                    drep: amaru_ledger::state::volatile::Resettable::Unchanged,
                    deposit: 0,
                    rewards: POOL_STAKE,
                },
            )
        })
        .collect();
    let issuer = pools[0].pool_id();
    let tx = db.create_transaction();
    tx.save(
        &network.era_history,
        &network.protocol_parameters,
        Some(GovernanceActivity { consecutive_dormant_epochs: 0 }),
        &anchor.point(),
        Some(&issuer),
        Columns {
            utxo: std::iter::empty(),
            pools: pool_rows.into_iter(),
            accounts: account_rows.into_iter(),
            dreps: std::iter::empty(),
            cc_members: std::iter::empty(),
            proposals: std::iter::empty(),
            votes: std::iter::empty(),
        },
        Columns::empty(),
        std::iter::empty(),
    )
    .expect("save stake");
    tx.commit().expect("stake commit");

    // Tip is in epoch 3. Leadership for epoch 4 reads the epoch-2 snapshot.
    for epoch in [Epoch::from(0), Epoch::from(1), Epoch::from(2)] {
        db.next_snapshot(epoch).expect("snapshot");
    }
}

fn write_chain(network: &MintNetwork, dir: &Path, parent: &Header, anchor: &Header) {
    let chain = RocksDBStore::create(RocksDbConfig::new(dir.to_path_buf())).expect("chain");
    store_block(&chain, parent, &network.era_history);
    chain.store_header(parent).expect("parent header");
    store_block(&chain, anchor, &network.era_history);
    let nonce = Nonce::from(ACTIVE_NONCE);
    let nonces = Nonces {
        active: nonce,
        evolving: nonce,
        candidate: nonce,
        tail: anchor.hash(),
        epoch: network.era_history.slot_to_epoch(anchor.slot(), anchor.slot()).expect("anchor epoch"),
    };
    chain.store_validated_header(anchor, &nonces).expect("anchor header");
    chain.set_block_valid(&parent.hash(), true).expect("parent valid");
    chain.set_block_valid(&anchor.hash(), true).expect("anchor valid");
    chain.set_anchor_point(&parent.point()).expect("anchor point");
    // The best-chain index is what chainsync intersection searches. The tip pointer alone is not enough.
    chain.roll_forward_chain(&parent.point()).expect("parent on chain");
    chain.roll_forward_chain(&anchor.point()).expect("anchor on chain");
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

fn node_log() -> (tracing::Dispatch, Arc<Mutex<Vec<LogRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let layer = Capture { records: records.clone() }.with_filter(tracing_subscriber::filter::filter_fn(|meta| {
        *meta.level() <= Level::WARN
            || (*meta.level() <= Level::DEBUG
                && matches!(
                    meta.name(),
                    "forge.forged"
                        | "forge.schedule"
                        | "forge.missed_slot"
                        | "forge.forge_failed"
                        | "perf.header.lifecycle"
                        | "tip.adopt"
                        | "tip.update"
                        | "rewards.summarize"
                ))
    }));
    let dispatch = tracing::Dispatch::new(registry().with(layer));
    (dispatch, records)
}

struct SyncWorld {
    seed: u64,
    handle: Handle,
    provider: Arc<WorldConnectionProvider>,
    _runtime: Runtime,
}

impl SyncWorld {
    fn new(label: &str) -> Self {
        let seed = draw_test_seed();
        eprintln!("world {label} seed={seed:#x}");
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
        let handle = runtime.handle().clone();
        let provider = Arc::new(WorldConnectionProvider::new(seed));
        Self { seed, handle, provider, _runtime: runtime }
    }
}

struct NodeSpawn {
    index: usize,
    listen: SocketAddr,
    upstream: Vec<Peer>,
    credentials: Arc<dyn amaru_ouroboros_traits::ForgingCredentials>,
    dispatch: tracing::Dispatch,
}

fn spawn_node(world: &SyncWorld, network: &MintNetwork, fixture: &MintFixture, node: NodeSpawn) -> SimulationRunning {
    let NodeSpawn { index, listen, upstream, credentials, dispatch } = node;
    let node_root = tempfile::tempdir().expect("node dir");
    let ledger = node_root.path().join("ledger");
    let chain = node_root.path().join("chain");
    copy_dir(&fixture.ledger_dir, &ledger).expect("copy ledger");
    copy_dir(&fixture.chain_dir, &chain).expect("copy chain");
    // Keep the directory alive for the node by leaking it into the graph via the path copy.
    // The TempDir must outlive the node; park it in a process-lifetime holder on the running graph
    // by forgetting it after the node has opened the databases. The test process exits after the run.
    let config = super::super::configuration::NodeTestConfig::default()
        .with_listen_address(&listen.to_string())
        .with_seed(derive_seed(world.seed, index as u64))
        .with_store_dirs(&chain, &ledger)
        .with_keep_persisted_best_chain()
        .with_no_upstream_peers()
        .with_upstream_peers(upstream)
        .with_target_upstream_peers(POOLS - 1)
        .with_peer_mix("static~4, inbound~4")
        .with_global_parameters(network.global_parameters.clone())
        .with_era_history(network.era_history.clone())
        .with_global_epoch_offset(Duration::from_secs(4 * network.global_parameters.epoch_length()))
        .with_forging_credentials(credentials);
    let running = tracing::dispatcher::with_default(&dispatch, || {
        build_world_node(&config, world.provider.clone(), &world.handle).expect("node")
    });
    std::mem::forget(node_root);
    running
}

fn listen(base: u16, index: usize) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], base + u16::try_from(index).expect("node index")))
}

/// Per-pool leadership probability at relative stake `1/5`.
fn leadership_probability() -> f64 {
    let active = 1.0 / ACTIVE_SLOT_COEFF_INVERSE;
    let sigma = 1.0 / POOLS as f64;
    1.0 - (1.0 - active).powf(sigma)
}

/// Probability a slot has at least one leader among `nodes` of the five equal pools.
fn occupancy(nodes: usize) -> f64 {
    let quiet = 1.0 - leadership_probability();
    1.0 - quiet.powi(i32::try_from(nodes).expect("node count"))
}

/// Probability two or more of the five pools lead the same slot.
fn battle_probability() -> f64 {
    let phi = leadership_probability();
    let quiet = 1.0 - phi;
    let none = quiet.powi(POOLS as i32);
    let one = POOLS as f64 * phi * quiet.powi((POOLS - 1) as i32);
    1.0 - none - one
}

/// Slots that cover `blocks` adoptions at `nodes` pools, plus fifteen standard deviations.
fn slots_for(blocks: u64, nodes: usize) -> u64 {
    let probability = occupancy(nodes);
    let mean = blocks as f64 / probability;
    let sd = (blocks as f64 * (1.0 - probability)).sqrt() / probability;
    (mean + 15.0 * sd).ceil() as u64
}

enum Until {
    Blocks(u64),
    Slot(u64),
}

struct MintRun {
    seed: u64,
    fixture: MintFixture,
    world: WorldLoop,
    logs: Vec<Arc<Mutex<Vec<LogRecord>>>>,
    security_param: u64,
}

fn run_mint(label: &str, nodes: usize, base_port: u16, until: Until, geometry: MintGeometry) -> MintRun {
    let world = SyncWorld::new(label);
    let security_param = geometry.security_param;
    let network = mint_network(geometry);
    let fixture = synthesize(&network);
    let chain_start = 4 * network.global_parameters.epoch_length();
    let anchor_height = fixture.anchor.block_height().as_u64();
    let mut graphs = Vec::new();
    let mut logs = Vec::new();
    for index in 0..nodes {
        let (dispatch, records) = node_log();
        let upstream = (index + 1..nodes).map(|peer| Peer::try_from(listen(base_port, peer)).expect("peer")).collect();
        let credentials: Arc<dyn amaru_ouroboros_traits::ForgingCredentials> = fixture.pools[index].clone();
        graphs.push(spawn_node(
            &world,
            &network,
            &fixture,
            NodeSpawn { index, listen: listen(base_port, index), upstream, credentials, dispatch: dispatch.clone() },
        ));
        logs.push((dispatch, records));
    }
    let mut loop_ = WorldLoop::new(world.provider.clone(), graphs);
    for (index, (dispatch, _)) in logs.iter().enumerate() {
        loop_.set_subscriber(index, dispatch.clone());
    }
    let horizon = match until {
        Until::Blocks(blocks) => slots_for(blocks, nodes),
        Until::Slot(slot) => slot - chain_start + slots_for(1, nodes),
    } * 1_000_000_000;
    loop_.run_until_horizon_on_best_chain_tip(horizon, |world| {
        let tips: Vec<_> = (0..nodes).map(|index| best_tip(world, index)).collect();
        let agreed = tips.windows(2).all(|pair| pair[0] == pair[1]);
        let reached = match until {
            Until::Blocks(blocks) => tips.iter().all(|tip| tip.block_height().as_u64() >= anchor_height + blocks),
            Until::Slot(slot) => tips.iter().all(|tip| u64::from(tip.slot_or_default()) >= slot),
        };
        if agreed && reached {
            world.halt();
        }
    });
    let records = logs.into_iter().map(|(_, records)| records).collect();
    MintRun { seed: world.seed, fixture, world: loop_, logs: records, security_param }
}

#[test]
fn synthesized_snapshot_splits_stake_evenly() {
    let network = mint_network(MintGeometry::standard());
    let fixture = synthesize(&network);
    let snapshots = RocksDBHistoricalStores::new(&RocksDbConfig::new(fixture.ledger_dir.clone()), 0);
    let distributions =
        initial_stake_distributions(NetworkName::Preprod, &snapshots, &network.era_history, false).expect("stake");
    let epoch2 = distributions.iter().find(|distribution| distribution.epoch == Epoch::from(2)).expect("epoch 2");
    assert_eq!(epoch2.pools.len(), POOLS, "five pools");
    let stakes: Vec<_> = epoch2.pools.values().map(|pool| pool.stake).collect();
    assert!(stakes.iter().all(|stake| *stake == stakes[0] && *stake > 0), "{stakes:?}");
    assert_eq!(epoch2.active_stake, stakes[0] * POOLS as u64);

    let mut by_epoch = BTreeMap::new();
    for distribution in &distributions {
        let pools = distribution
            .pools
            .iter()
            .map(|(id, state)| {
                (
                    *id,
                    PoolSummary {
                        vrf: state.parameters.vrf,
                        stake: state.stake,
                        active_stake: distribution.active_stake,
                    },
                )
            })
            .collect();
        by_epoch.insert(distribution.epoch, pools);
    }
    let summaries = PoolSummaries { by_epoch };
    let slot = Slot::from(4 * network.global_parameters.epoch_length());
    for credentials in &fixture.pools {
        let summary =
            summaries.get_pool(slot, &credentials.pool_id(), &network.era_history).expect("lookup").expect("pool");
        assert_eq!(summary.stake * POOLS as u64, summary.active_stake);
        let vrf = Hasher::<256>::hash(credentials.vrf_verification_key().as_slice());
        assert_eq!(summary.vrf, vrf);
    }
}

#[test]
fn test_world_one_node_forges_one_block() {
    let run = run_mint("one_node_one_block", 1, 15_010, Until::Blocks(1), MintGeometry::standard());
    let tip = best_tip(&run.world, 0);
    let logs = snapshot(&run.logs);
    assert!(
        tip.block_height() > run.fixture.anchor.block_height(),
        "tip={tip} seed={:#x} {}",
        run.seed,
        dump_other(&logs)
    );
    assert!(
        events(&logs[0], "forge.forged").next().is_some(),
        "missing forge.forged seed={:#x} {}",
        run.seed,
        dump_names(&logs)
    );
    assert_no_forge_failures(run.seed, &logs);
}

#[test]
fn test_world_one_node_mints_across_epoch_boundary() {
    let epoch5 = 5 * MintGeometry::standard().epoch_length();
    let run = run_mint("one_node_epoch", 1, 15_020, Until::Slot(epoch5), MintGeometry::standard());
    let tip = best_tip(&run.world, 0);
    let logs = snapshot(&run.logs);
    assert!(u64::from(tip.slot_or_default()) >= epoch5, "tip={tip} seed={:#x} {}", run.seed, dump_names(&logs));
    assert!(
        events(&logs[0], "rewards.summarize").next().is_some(),
        "missing rewards.summarize seed={:#x} {}",
        run.seed,
        dump_names(&logs)
    );
    assert!(
        events(&logs[0], "forge.schedule")
            .any(|record| { record.fields.get("slots").is_some_and(|slots| schedule_has_epoch(slots, 5)) }),
        "missing epoch-5 schedule seed={:#x} {}",
        run.seed,
        events(&logs[0], "forge.schedule")
            .filter_map(|record| record.fields.get("slots"))
            .next()
            .cloned()
            .unwrap_or_default()
    );
    assert_no_forge_failures(run.seed, &logs);
}

#[test]
fn test_world_five_nodes_mint_smoke() {
    let run = run_mint("five_node_smoke", POOLS, 15_030, Until::Blocks(SMOKE_BLOCKS), MintGeometry::standard());
    assert_minted_chain(&run, SMOKE_BLOCKS);
}

#[test]
#[ignore = "1_000 blocks on the 50_000-slot epoch"]
fn test_world_five_nodes_mint() {
    let run = run_mint("five_node_mint", POOLS, 15_040, Until::Blocks(LONG_BLOCKS), MintGeometry::standard());
    assert_minted_chain(&run, LONG_BLOCKS);
}

/// Four short epochs: `k = 25`, epoch length 2_500, nonce freeze 2_000. The KES period is
/// 5_000 slots, so the run evolves the operational certificate.
#[test]
fn test_world_five_nodes_mint_short_epochs() {
    let geometry = MintGeometry::short_epochs();
    let target = geometry.chain_start_slot() + SHORT_EPOCHS * geometry.epoch_length();
    let run = run_mint("five_node_short_epochs", POOLS, 15_050, Until::Slot(target), geometry);
    let tip = best_tip(&run.world, 0);
    let added = tip.block_height().as_u64() - run.fixture.anchor.block_height().as_u64();
    assert!(u64::from(tip.slot_or_default()) >= target, "tip={tip} seed={:#x}", run.seed);
    assert_minted_chain(&run, added);

    let logs = snapshot(&run.logs);
    assert!(
        logs.iter().any(|records| events(records, "rewards.summarize").next().is_some()),
        "missing rewards.summarize seed={:#x}",
        run.seed
    );
    // chain_start is epoch 4; four epochs later the tip has entered epoch 8.
    assert!(
        logs.iter().any(|records| {
            events(records, "forge.schedule")
                .any(|record| record.fields.get("slots").is_some_and(|slots| schedule_has_epoch(slots, 8)))
        }),
        "missing epoch-8 schedule seed={:#x}",
        run.seed
    );

    // The certificate's start period stays at issuance. The slot's period is what the
    // signature had to evolve to, and validation accepted that header.
    let slots_per_period = MintGeometry::short_epochs().slots_per_kes_period;
    let anchor_period = u64::from(run.fixture.anchor.slot()) / slots_per_period;
    let tip_period = u64::from(tip.slot_or_default()) / slots_per_period;
    assert!(
        tip_period > anchor_period,
        "KES period did not advance: anchor={anchor_period} tip={tip_period} seed={:#x}",
        run.seed
    );
}

fn assert_minted_chain(run: &MintRun, blocks: u64) {
    let seed = run.seed;
    let anchor = &run.fixture.anchor;
    let anchor_height = anchor.block_height().as_u64();
    let anchor_slot = u64::from(anchor.slot());
    let logs = snapshot(&run.logs);
    let tips: Vec<_> = (0..logs.len()).map(|index| best_tip(&run.world, index)).collect();
    assert!(
        tips.windows(2).all(|pair| pair[0] == pair[1]),
        "tips disagree seed={seed:#x} tips={tips:?} {} {}",
        connection_summary(&run.world),
        rejection_dump(&logs)
    );
    let tip = tips[0];
    assert_eq!(
        tip.block_height().as_u64(),
        anchor_height + blocks,
        "height seed={seed:#x} tip={tip} {}",
        dump_names(&logs)
    );
    let span = u64::from(tip.slot_or_default()) - anchor_slot;
    assert!(span > 0, "span seed={seed:#x} tip={tip}");

    let stores: Vec<_> = (0..logs.len()).map(|index| chain_store(&run.world, index)).collect();
    let walked: Vec<_> =
        stores.iter().map(|store| ancestor_hashes(store.as_ref(), tip.hash(), run.security_param as usize)).collect();
    assert!(walked.windows(2).all(|pair| pair[0] == pair[1]), "k headers disagree seed={seed:#x}");

    assert_no_forge_failures(seed, &logs);
    assert_density(seed, blocks, span, anchor_height, run.security_param, &logs);
    let chain = adopted_chain(stores[0].as_ref(), tip.hash(), anchor.hash());
    assert_eq!(chain.len(), blocks as usize, "adopted headers seed={seed:#x}");
    assert_battles(seed, span, &logs, &chain, anchor);
    assert_forwarding(seed, &run.fixture, &logs, &chain, anchor);
    assert_schedules(seed, &logs);
}

fn assert_no_forge_failures(seed: u64, logs: &[Vec<LogRecord>]) {
    for records in logs {
        assert!(
            events(records, "forge.forge_failed").next().is_none(),
            "FORGE_FAILED seed={seed:#x} {}",
            dump_names(logs)
        );
        assert!(
            events(records, "forge.missed_slot").next().is_none(),
            "MISSED_SLOT seed={seed:#x} {}",
            dump_names(logs)
        );
    }
}

fn assert_density(seed: u64, blocks: u64, span: u64, anchor_height: u64, security_param: u64, logs: &[Vec<LogRecord>]) {
    let active = 1.0 / ACTIVE_SLOT_COEFF_INVERSE;
    let ratio = blocks as f64 / span as f64;
    let sd = active * ((1.0 - active) / blocks as f64).sqrt();
    assert!(
        (ratio - active).abs() <= 4.0 * sd,
        "density {ratio} vs {active} ± {sd} seed={seed:#x} blocks={blocks} span={span}"
    );

    // Four standard deviations of the slot span of k blocks, mapped back through
    // density = k / span. The upper side is wider because that ratio is skewed.
    let blocks_in_window = security_param as f64;
    let mean_span = blocks_in_window / active;
    let span_sd = (blocks_in_window * (1.0 - active)).sqrt() / active;
    let shortest = (mean_span - 4.0 * span_sd).max(blocks_in_window);
    let longest = mean_span + 4.0 * span_sd;
    let band_lo = (blocks_in_window / longest) / active;
    let band_hi = (blocks_in_window / shortest) / active;
    let floor = anchor_height + security_param;
    let mut latest = Vec::new();
    for (index, records) in logs.iter().enumerate() {
        let mut samples = Vec::new();
        for record in events(records, "tip.update") {
            let Some(height) = field_u64(record, "block_height") else {
                continue;
            };
            if height <= floor {
                continue;
            }
            let density = field_f64(record, "density")
                .unwrap_or_else(|| panic!("density field seed={seed:#x} node={index} {:?}", record.fields));
            let scaled = density * ACTIVE_SLOT_COEFF_INVERSE;
            assert!(
                (band_lo..=band_hi).contains(&scaled),
                "k-window {scaled} not in [{band_lo}, {band_hi}] seed={seed:#x} node={index} height={height}"
            );
            samples.push((height, density));
        }
        if let Some(sample) = samples.last().copied() {
            latest.push(sample);
        }
    }
    // Debounce means the latest sample on each node can be a different block.
    // The same height is the same window, so those densities match.
    if let Some(&(height, _)) = latest.iter().max_by_key(|(height, _)| height) {
        let agreed: Vec<f64> =
            latest.iter().filter(|(sample_height, _)| *sample_height == height).map(|(_, density)| *density).collect();
        let first = agreed[0];
        assert!(
            agreed.iter().all(|density| (density - first).abs() <= 1e-9),
            "density at height {height}: {agreed:?} seed={seed:#x}"
        );
    }
}

fn assert_battles(seed: u64, span: u64, logs: &[Vec<LogRecord>], chain: &[Header], anchor: &Header) {
    let by_slot = forged_by_slot(logs);
    let battles = by_slot
        .values()
        .filter(|forged| {
            let hashes: BTreeSet<_> = forged.iter().map(|(_, hash, _)| hash.clone()).collect();
            hashes.len() >= 2
        })
        .count();
    let probability = battle_probability();
    let mean = span as f64 * probability;
    let sd = (span as f64 * probability * (1.0 - probability)).sqrt();
    assert!(
        (battles as f64 - mean).abs() <= 4.0 * sd,
        "battles {battles} expected {mean:.1} ± {sd:.1} seed={seed:#x} span={span}"
    );

    let on_chain: BTreeSet<_> = chain.iter().map(|header| header.hash().to_string()).collect();
    let mut previous = anchor.hash().to_string();
    for header in chain {
        let hash = header.hash().to_string();
        let slot = u64::from(header.slot());
        let forged = by_slot.get(&slot).map(Vec::as_slice).unwrap_or(&[]);
        let distinct: BTreeSet<_> = forged.iter().map(|(_, forged_hash, _)| forged_hash.clone()).collect();
        if distinct.len() >= 2 {
            for (_, forged_hash, parent) in forged {
                assert_eq!(parent, &previous, "battle parent seed={seed:#x} slot={slot}");
                if forged_hash != &hash {
                    assert!(!on_chain.contains(forged_hash), "losing header on chain seed={seed:#x} {forged_hash}");
                }
            }
            let adopted = distinct.iter().filter(|candidate| on_chain.contains(*candidate)).count();
            assert_eq!(adopted, 1, "battle resolution seed={seed:#x} slot={slot} {distinct:?}");
        } else {
            assert_eq!(distinct.len(), 1, "forgers at slot {slot} seed={seed:#x} {distinct:?}");
            assert!(distinct.contains(&hash), "adopted header not forged seed={seed:#x} slot={slot} {hash}");
            assert_eq!(forged[0].2, previous, "parent seed={seed:#x} slot={slot}");
        }
        previous = hash;
    }
}

fn assert_forwarding(seed: u64, fixture: &MintFixture, logs: &[Vec<LogRecord>], chain: &[Header], anchor: &Header) {
    let mut previous = anchor.hash().to_string();
    for header in chain {
        let hash = header.hash().to_string();
        let mut forgers = Vec::new();
        for (index, records) in logs.iter().enumerate() {
            let forged: Vec<_> = events(records, "forge.forged")
                .filter(|record| field_hex(record, "header_hash").as_deref() == Some(hash.as_str()))
                .collect();
            if forged.is_empty() {
                continue;
            }
            assert_eq!(forged.len(), 1, "duplicate forge.forged seed={seed:#x} node={index} {hash}");
            assert_eq!(field_u64(forged[0], "slot"), Some(u64::from(header.slot())), "{:?}", forged[0].fields);
            assert_eq!(field_hex(forged[0], "parent").as_deref(), Some(previous.as_str()), "{:?}", forged[0].fields);
            forgers.push(index);
        }
        assert_eq!(forgers.len(), 1, "forgers {forgers:?} seed={seed:#x} {hash}");
        let forger = forgers[0];
        assert_eq!(header.pool_id(), fixture.pools[forger].pool_id(), "issuer seed={seed:#x} {hash}");

        for (index, records) in logs.iter().enumerate() {
            let lifecycles: Vec<_> = events(records, "perf.header.lifecycle")
                .filter(|record| field_hex(record, "header_hash").as_deref() == Some(hash.as_str()))
                .collect();
            let valid = lifecycles.iter().filter(|record| field_str(record, "outcome") == Some("valid")).count();
            if index == forger {
                assert_eq!(valid, 0, "forger logged a peer lifecycle seed={seed:#x} {hash}");
            } else {
                assert_eq!(valid, 1, "valid lifecycles {valid} seed={seed:#x} node={index} {hash}");
                let record =
                    lifecycles.iter().find(|record| field_str(record, "outcome") == Some("valid")).expect("valid");
                assert!(
                    field_u64(record, "forward_micros").is_some(),
                    "forward_micros seed={seed:#x} {:?}",
                    record.fields
                );
            }
            for record in &lifecycles {
                let outcome = field_str(record, "outcome");
                assert!(
                    outcome == Some("valid") || outcome == Some("duplicate_header") || outcome == Some("pruned"),
                    "lifecycle {outcome:?} seed={seed:#x} {:?}",
                    record.fields
                );
            }
            let adopted = events(records, "tip.adopt")
                .any(|record| field_hex(record, "header_hash").as_deref() == Some(hash.as_str()));
            assert!(adopted, "missing tip.adopt seed={seed:#x} node={index} {hash}");
        }
        previous = hash;
    }
}

fn assert_schedules(seed: u64, logs: &[Vec<LogRecord>]) {
    for (index, records) in logs.iter().enumerate() {
        let scheduled = events(records, "forge.schedule")
            .any(|record| record.fields.get("slots").is_some_and(|slots| !slots.is_empty() && slots != "{}"));
        assert!(scheduled, "SCHEDULE seed={seed:#x} node={index} {}", schedule_dump(records));
    }
}

fn forged_by_slot(logs: &[Vec<LogRecord>]) -> BTreeMap<u64, Vec<(usize, String, String)>> {
    let mut by_slot: BTreeMap<u64, Vec<(usize, String, String)>> = BTreeMap::new();
    for (index, records) in logs.iter().enumerate() {
        for record in events(records, "forge.forged") {
            let slot = field_u64(record, "slot").unwrap_or_else(|| panic!("slot {:?}", record.fields));
            let hash = field_hex(record, "header_hash").unwrap_or_else(|| panic!("header_hash {:?}", record.fields));
            let parent = field_hex(record, "parent").unwrap_or_else(|| panic!("parent {:?}", record.fields));
            by_slot.entry(slot).or_default().push((index, hash, parent));
        }
    }
    by_slot
}

fn rejection_dump(logs: &[Vec<LogRecord>]) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    for records in logs {
        for record in records {
            let message = record.fields.get("message").map(String::as_str).unwrap_or("");
            let interesting = record.name.contains("invalid")
                || record.name.contains("fail")
                || record.name.contains("ban")
                || record.name.contains("adversar")
                || record.name.contains("disconnect")
                || record.name.contains("refused")
                || record.name.contains("child")
                || record.name.contains("chainsync")
                || record.name == "event"
                || message.contains("fail");
            if !interesting {
                continue;
            }
            let key = if record.name == "event" { message.to_string() } else { record.name.clone() };
            *counts.entry(key.clone()).or_default() += 1;
            examples.entry(key).or_insert_with(|| {
                let operation = record.fields.get("operation").map(String::as_str).unwrap_or("");
                let error =
                    record.fields.get("error").or_else(|| record.fields.get("err")).map(String::as_str).unwrap_or("");
                let child = record.fields.get("child").map(String::as_str).unwrap_or("");
                format!("op={operation} err={error} child={child} target={}", record.target)
            });
        }
    }
    format!("counts={counts:?} examples={examples:?}")
}

fn connection_summary(world: &WorldLoop) -> String {
    use super::HeapLogKind;
    let log = world.heap_log();
    let count = |pred: fn(&HeapLogKind) -> bool| log.iter().filter(|entry| pred(&entry.kind)).count();
    format!(
        "connects={} accepts={} timeouts={} delivers={} closes={}",
        count(|kind| matches!(kind, HeapLogKind::ConnectAttempt { .. })),
        count(|kind| matches!(kind, HeapLogKind::Accepted { .. })),
        count(|kind| matches!(kind, HeapLogKind::ConnectTimeout { .. })),
        count(|kind| matches!(kind, HeapLogKind::Deliver { .. })),
        count(|kind| matches!(kind, HeapLogKind::Close { .. })),
    )
}

fn best_tip(world: &WorldLoop, index: usize) -> Point {
    chain_store(world, index).get_best_chain_tip()
}

fn chain_store(world: &WorldLoop, index: usize) -> ResourceHeaderStore {
    let store = world.graphs()[index].resources().get::<ResourceHeaderStore>().expect("chain store");
    Arc::clone(&store)
}

fn ancestor_hashes(store: &dyn ChainStore, tip: Hash<32>, count: usize) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut hash = tip;
    for _ in 0..count {
        hashes.push(hash.to_string());
        let Some(header) = store.load_header(&hash) else {
            break;
        };
        let Some(parent) = header.parent() else {
            break;
        };
        hash = parent;
    }
    hashes
}

fn adopted_chain(store: &dyn ChainStore, tip: Hash<32>, anchor: Hash<32>) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut hash = tip;
    while hash != anchor {
        let header = store.load_header(&hash).unwrap_or_else(|| panic!("missing header {hash}"));
        let parent = header.parent().unwrap_or_else(|| panic!("header {hash} has no parent"));
        headers.push(header);
        hash = parent;
    }
    headers.reverse();
    headers
}

fn snapshot(logs: &[Arc<Mutex<Vec<LogRecord>>>]) -> Vec<Vec<LogRecord>> {
    logs.iter().map(|log| log.lock().expect("log").clone()).collect()
}

fn events<'a>(records: &'a [LogRecord], name: &'a str) -> impl Iterator<Item = &'a LogRecord> + 'a {
    records.iter().filter(move |record| record.name == name)
}

fn field_str<'a>(record: &'a LogRecord, key: &str) -> Option<&'a str> {
    record.fields.get(key).map(|value| value.trim().trim_matches('"'))
}

fn field_u64(record: &LogRecord, key: &str) -> Option<u64> {
    let value = record.fields.get(key)?;
    if let Some(bytes) = cbor_bytes(value) {
        return decode_cbor_uint(&bytes);
    }
    value.trim().trim_matches('"').parse().ok()
}

fn field_f64(record: &LogRecord, key: &str) -> Option<f64> {
    let value = record.fields.get(key)?;
    if let Some(bytes) = cbor_bytes(value)
        && let [0xfb, rest @ ..] = bytes.as_slice()
        && rest.len() == 8
    {
        return Some(f64::from_be_bytes(rest.try_into().ok()?));
    }
    value.trim().trim_matches('"').parse().ok()
}

fn field_hex(record: &LogRecord, key: &str) -> Option<String> {
    let value = record.fields.get(key)?;
    let bytes = cbor_bytes(value)?;
    match bytes.as_slice() {
        [0x58, 32, hash @ ..] if hash.len() == 32 => Some(encode_hex(hash)),
        _ => None,
    }
}

/// Observability renders typed fields as the hex bytes of their CBOR encoding.
fn cbor_bytes(value: &str) -> Option<Vec<u8>> {
    let inside = value.trim().trim_matches(|c| c == '[' || c == ']' || c == '"');
    let mut bytes = Vec::new();
    for token in inside.split_whitespace() {
        if token.len() != 2 {
            return None;
        }
        bytes.push(u8::from_str_radix(token, 16).ok()?);
    }
    Some(bytes)
}

fn decode_cbor_uint(bytes: &[u8]) -> Option<u64> {
    match bytes {
        [n] if *n < 24 => Some(u64::from(*n)),
        [0x18, n] => Some(u64::from(*n)),
        [0x19, a, b] => Some(u64::from(u16::from_be_bytes([*a, *b]))),
        [0x1a, a, b, c, d] => Some(u64::from(u32::from_be_bytes([*a, *b, *c, *d]))),
        [0x1b, a, b, c, d, e, f, g, h] => Some(u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn dump_names(logs: &[Vec<LogRecord>]) -> String {
    logs.iter()
        .enumerate()
        .map(|(index, records)| {
            format!(
                "node {index}: {}",
                records
                    .iter()
                    .map(|record| format!("{} {}", record.target, record.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn dump_other(logs: &[Vec<LogRecord>]) -> String {
    logs.iter()
        .enumerate()
        .map(|(index, records)| {
            let lines: Vec<_> = records
                .iter()
                .filter(|record| record.name != "forge.schedule" && record.name != "forge.forged")
                .map(|record| format!("{} {:?}", record.name, record.fields))
                .collect();
            format!("node {index}: {}", lines.join(" || "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `slots` is a CBOR map from epoch to remaining lead count.
fn schedule_has_epoch(slots: &str, epoch: u64) -> bool {
    let Some(bytes) = cbor_bytes(slots) else {
        return false;
    };
    let Some((&header, rest)) = bytes.split_first() else {
        return false;
    };
    if !(0xa0..0xb8).contains(&header) {
        return false;
    }
    let mut index = 0;
    for _ in 0..(header - 0xa0) {
        let Some((key, next)) = read_cbor_uint_at(rest, index) else {
            return false;
        };
        if key == epoch {
            return true;
        }
        let Some((_, next)) = read_cbor_uint_at(rest, next) else {
            return false;
        };
        index = next;
    }
    false
}

fn read_cbor_uint_at(bytes: &[u8], index: usize) -> Option<(u64, usize)> {
    let head = *bytes.get(index)?;
    match head {
        0..=23 => Some((u64::from(head), index + 1)),
        0x18 => Some((u64::from(*bytes.get(index + 1)?), index + 2)),
        0x19 => {
            let value = u16::from_be_bytes([*bytes.get(index + 1)?, *bytes.get(index + 2)?]);
            Some((u64::from(value), index + 3))
        }
        0x1a => {
            let value = u32::from_be_bytes([
                *bytes.get(index + 1)?,
                *bytes.get(index + 2)?,
                *bytes.get(index + 3)?,
                *bytes.get(index + 4)?,
            ]);
            Some((u64::from(value), index + 5))
        }
        _ => None,
    }
}

fn schedule_dump(records: &[LogRecord]) -> String {
    events(records, "forge.schedule")
        .filter_map(|record| record.fields.get("slots").cloned())
        .collect::<Vec<_>>()
        .join("; ")
}
