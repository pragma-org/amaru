// Copyright 2024 PRAGMA
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

use amaru_kernel::{
    default_ledger_dir, network::NetworkName, protocol_parameters::ProtocolParameters, Anchor,
    CertificatePointer, DRep, EraHistory, Lovelace, Point, PoolId, PoolParams, Proposal,
    ProposalId, ProposalPointer, Set, Slot, StakeCredential, TransactionInput, TransactionOutput,
    TransactionPointer,
};
use amaru_ledger::{
    self,
    state::diff_bind::Resettable,
    store::{
        self, columns::proposals, EpochTransitionProgress, Store, StoreError, TransactionalContext,
    },
};
use amaru_stores::rocksdb::RocksDB;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use pallas_codec::minicbor as cbor;
use slot_arithmetic::Epoch;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, iter,
    path::PathBuf,
    sync::LazyLock,
};
use tracing::info;

const BATCH_SIZE: usize = 5000;

static DEFAULT_CERTIFICATE_POINTER: LazyLock<CertificatePointer> =
    LazyLock::new(|| CertificatePointer {
        transaction: TransactionPointer {
            slot: Slot::from(0),
            transaction_index: 0,
        },
        certificate_index: 0,
    });

#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the CBOR snapshot. The snapshot can be obtained from the Haskell
    /// cardano-node, using the `DebugEpochState` command, serialised as CBOR.
    ///
    /// The snapshot must be named after the point on-chain it is reflecting, as
    ///
    /// `  {SLOT}.{BLOCK_HEADER_HASH}.cbor`
    ///
    /// For example:
    ///
    ///   68774372.36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor
    ///
    /// Can be repeated multiple times for multiple snapshots.
    #[arg(long, value_name = "SNAPSHOT", verbatim_doc_comment, num_args(0..))]
    snapshot: Vec<PathBuf>,
    /// Path to a directory containing multiple CBOR snapshots to import.
    #[arg(long, value_name = "DIR")]
    snapshot_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Network the snapshots are imported from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("malformed date: {}", .0)]
    MalformedDate(String),
    #[error("You must provide either a single .cbor snapshot file (--snapshot) or a directory containing multiple .cbor snapshots (--snapshot-dir)")]
    IncorrectUsage,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let era_history = network.into();
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(args.network).into());
    if !args.snapshot.is_empty() {
        import_all(&args.snapshot, &ledger_dir, era_history).await
    } else if let Some(snapshot_dir) = args.snapshot_dir {
        import_all_from_directory(&ledger_dir, era_history, &snapshot_dir).await
    } else {
        Err(Error::IncorrectUsage.into())
    }
}

pub(crate) async fn import_all_from_directory(
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
    snapshot_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut snapshots = fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("cbor"))
        .collect::<Vec<_>>();

    sort_snapshots_by_slot(&mut snapshots);

    import_all(&snapshots, ledger_dir, era_history).await
}

fn sort_snapshots_by_slot(snapshots: &mut [PathBuf]) {
    // Sort by parsed slot number from filename
    snapshots.sort_by_key(|path| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('.').next())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });

    snapshots.sort();
}

async fn import_all(
    snapshots: &Vec<PathBuf>,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing {} snapshots", snapshots.len());
    for snapshot in snapshots {
        import_one(snapshot, ledger_dir, era_history).await?;
    }
    Ok(())
}

#[allow(clippy::unwrap_used)]
async fn import_one(
    snapshot: &PathBuf,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing snapshot {}", snapshot.display());
    let point = super::parse_point(
        snapshot
            .as_path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap(),
    )
    .map_err(Error::MalformedDate)?;

    fs::create_dir_all(ledger_dir)?;
    let db = RocksDB::empty(ledger_dir, era_history)?;
    let bytes = fs::read(snapshot)?;

    let epoch = decode_new_epoch_state(&db, &bytes, &point, era_history)?;
    let transaction = db.create_transaction();
    transaction.save(
        &point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
        BTreeSet::new(),
    )?;
    transaction.commit()?;

    db.next_snapshot(epoch)?;

    let transaction = db.create_transaction();
    transaction.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken))?;
    transaction.commit()?;

    info!("Imported snapshot for epoch {}", epoch);
    Ok(())
}

fn decode_new_epoch_state(
    db: &(impl Store + 'static),
    bytes: &[u8],
    point: &Point,
    era_history: &EraHistory,
) -> Result<Epoch, Box<dyn std::error::Error>> {
    let mut d = cbor::Decoder::new(bytes);

    d.array()?;

    // EpochNo
    let epoch = Epoch::from(d.u64()?);
    assert_eq!(epoch, era_history.slot_to_epoch(point.slot_or_default())?);

    // Previous blocks made
    d.skip()?;

    // Current blocks made
    // NOTE: We use the current blocks made here as we assume that users are providing snapshots of
    // the last block of the epoch. We have no intrinsic ways to check that this is the case since
    // we do not know what the last block of an epoch is, and we can't reliably look at the number
    // of blocks either.
    import_block_issuers(db, d.decode()?)?;

    // Epoch State
    d.array()?;

    // Epoch State / Account State
    d.array()?;
    let treasury: i64 = d.decode()?;
    let reserves: i64 = d.decode()?;

    // Epoch State / Ledger State
    d.array()?;

    // Epoch State / Ledger State / Cert State
    d.array()?;

    // Epoch State / Ledger State / Cert State / Voting State
    d.array()?;

    let dreps = d.decode()?;

    // Committee
    d.skip()?;

    // Dormant Epoch
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Pool State
    d.array()?;
    import_stake_pools(
        db,
        point,
        epoch,
        // Pools
        d.decode()?,
        // Updates
        d.decode()?,
        // Retirements
        d.decode()?,
    )?;
    // Deposits
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state
    d.array()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsUnified
    d.array()?;

    // credentials
    let accounts: BTreeMap<StakeCredential, Account> = d.decode()?;

    // pointers
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsFutureGenDelegs
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsGenDelegs
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsIRewards
    d.skip()?;

    // Epoch State / Ledger State / UTxO State
    d.array()?;

    import_utxo(
        db,
        point,
        d.decode::<BTreeMap<TransactionInput, TransactionOutput>>()?
            .into_iter()
            .collect::<Vec<(TransactionInput, TransactionOutput)>>(),
    )?;

    let _deposited: u64 = d.decode()?;

    let fees: i64 = d.decode()?;

    // Epoch State / Ledger State / UTxO State / utxosGovState
    d.array()?;

    // Proposals
    d.array()?;
    // Proposals roots
    d.skip()?;
    let proposals: Vec<ProposalState> = d.decode()?;

    // Constitutional committee
    d.skip()?;
    // Constitution
    d.skip()?;
    // Current Protocol Params
    let protocol_parameters = import_protocol_parameters(db, &epoch, d.decode()?)?;
    import_dreps(db, era_history, point, epoch, dreps, &protocol_parameters)?;
    import_proposals(db, point, era_history, proposals, &protocol_parameters)?;

    // Previous Protocol Params
    d.skip()?;
    // Future Protocol Params
    d.skip()?;
    // DRep Pulsing State
    d.skip()?;

    // Epoch State / Ledger State / UTxO State / utxosStakeDistr
    d.skip()?;

    // Epoch State / Ledger State / UTxO State / utxosDonation
    d.skip()?;

    // Epoch State / Snapshots
    d.skip()?;
    // Epoch State / NonMyopic
    d.skip()?;

    // Rewards Update
    d.array()?;
    d.array()?;
    assert_eq!(d.u32()?, 1, "expected complete pulsing reward state");
    d.array()?;

    let delta_treasury: i64 = d.decode()?;

    let delta_reserves: i64 = d.decode()?;

    let mut rewards: BTreeMap<StakeCredential, Set<Reward>> = d.decode()?;
    let delta_fees: i64 = d.decode()?;

    // NonMyopic
    d.skip()?;

    import_accounts(db, point, accounts, &mut rewards, &protocol_parameters)?;

    let unclaimed_rewards = rewards.into_iter().fold(0, |total, (_, rewards)| {
        total + rewards.into_iter().fold(0, |inner, r| inner + r.amount)
    });

    import_pots(
        db,
        (treasury + delta_treasury) as u64 + unclaimed_rewards,
        (reserves - delta_reserves) as u64,
        (fees - delta_fees) as u64,
    )?;

    Ok(epoch)
}

fn import_protocol_parameters(
    db: &impl Store,
    epoch: &Epoch,
    protocol_parameters: ProtocolParameters,
) -> Result<ProtocolParameters, Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.set_protocol_parameters(epoch, &protocol_parameters)?;
    transaction.commit()?;
    Ok(protocol_parameters)
}

fn import_block_issuers(
    db: &impl Store,
    blocks: BTreeMap<PoolId, u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_block_issuers(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;
    transaction.commit()?;

    let transaction = db.create_transaction();
    let mut fake_slot = 0;
    for (pool, mut count) in blocks.into_iter() {
        while count > 0 {
            transaction.save(
                &Point::Specific(fake_slot, vec![]),
                Some(&pool),
                store::Columns {
                    utxo: iter::empty(),
                    pools: iter::empty(),
                    accounts: iter::empty(),
                    dreps: iter::empty(),
                    cc_members: iter::empty(),
                    proposals: iter::empty(),
                },
                Default::default(),
                iter::empty(),
                BTreeSet::new(),
            )?;
            count -= 1;
            fake_slot += 1;
        }
    }
    info!(what = "block_issuers", count = fake_slot);
    transaction.commit().map_err(Into::into)
}

fn import_utxo(
    db: &impl Store,
    point: &Point,
    mut utxo: Vec<(TransactionInput, TransactionOutput)>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(what = "utxo_entries", size = utxo.len());

    let progress_delete = ProgressBar::no_length().with_style(ProgressStyle::with_template(
        "  Pruning UTxO entries {spinner} {elapsed}",
    )?);

    let transaction = db.create_transaction();
    transaction.with_utxo(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
            progress_delete.tick();
        }
    })?;

    progress_delete.finish_and_clear();

    let progress = ProgressBar::new(utxo.len() as u64).with_style(ProgressStyle::with_template(
        "  UTxO entries {bar:70} {pos:>7}/{len:7}",
    )?);

    while !utxo.is_empty() {
        let n = std::cmp::min(BATCH_SIZE, utxo.len());
        let chunk = utxo.drain(0..n);

        transaction.save(
            point,
            None,
            store::Columns {
                utxo: chunk,
                pools: iter::empty(),
                accounts: iter::empty(),
                dreps: iter::empty(),
                cc_members: iter::empty(),
                proposals: iter::empty(),
            },
            Default::default(),
            iter::empty(),
            BTreeSet::new(),
        )?;

        progress.inc(n as u64);
    }
    transaction.commit()?;
    progress.finish_and_clear();

    Ok(())
}

fn import_dreps(
    db: &impl Store,
    era_history: &EraHistory,
    point: &Point,
    epoch: Epoch,
    dreps: BTreeMap<StakeCredential, DRepState>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), impl std::error::Error> {
    let mut known_dreps = BTreeMap::new();

    let era_first_epoch = era_history
        .era_first_epoch(epoch)
        .map_err(|e| StoreError::Internal(Box::new(e)))?;

    let transaction = db.create_transaction();
    transaction.with_dreps(|iterator| {
        for (drep, mut handle) in iterator {
            if epoch > era_first_epoch {
                if let Some(row) = handle.borrow() {
                    known_dreps.insert(drep, row.registered_at);
                }
            }
            *handle.borrow_mut() = None;
        }
    })?;

    info!(what = "dreps", size = dreps.len());

    transaction.save(
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: dreps.into_iter().map(|(credential, state)| {
                // 1. First DRep registrations in Conway are *sometimes* granted an extra epoch of
                //    expiry; because the first Conway epoch is deemed as "dormant" (no proposals
                //    in the epoch prior), and a bug in version 9 is causing new DRep registrations
                //    to benefits from this extra epoch.
                //
                // 2. We have no idea when exactly was the drep registered; but we
                //    need to pick a valid slot so that mandate calculations falls
                //    back on the correct value.
                //
                //    There are two scenarios:
                //
                //    A) Either the drep has registered before the first proposal in
                //       the epoch. In which case it would enjoy an extra epoch of
                //       expiry.
                //
                //    B) Or it has registered strictly after, such that the number of
                //       dormant epoch was already reset. In which case, no bonus
                //       applies.
                //
                //    We can assign dreps to (A) or (B) by artificially chosing the
                //    first and last slot of the epoch respectively. To know whether
                //    we shall assign them to (A) or (B), we can simply look at their
                //    mandate in the snapshot which would be one greater for dreps in
                //    group (A).
                //
                // 3. We make a strong assumption that there are proposals submitted during the
                //    very first epoch of the Conway era on this network. This is true of Preview,
                //    Preprod and Mainnet. Any custom network for which this wouldn't be true is
                //    expected to use a protocol version > 9, where this assumption doesn't matter.
                #[allow(clippy::unwrap_used)]
                let (registration_slot, last_interaction) = if epoch == era_first_epoch {
                    let last_interaction = era_first_epoch;
                    let epoch_bound = era_history.epoch_bounds(last_interaction).unwrap();
                    if state.expiry > epoch + protocol_parameters.drep_expiry as u64 {
                        (epoch_bound.start, last_interaction)
                    } else {
                        (point.slot_or_default(), last_interaction)
                    }
                } else {
                    let last_interaction = state.expiry - protocol_parameters.drep_expiry as u64;
                    let epoch_bound = era_history.epoch_bounds(last_interaction).unwrap();
                    // start or end doesn't matter here.
                    (epoch_bound.start, last_interaction)
                };

                let registration =
                    known_dreps
                        .remove(&credential)
                        .unwrap_or_else(|| CertificatePointer {
                            transaction: TransactionPointer {
                                slot: registration_slot,
                                ..TransactionPointer::default()
                            },
                            ..CertificatePointer::default()
                        });

                (
                    credential,
                    (
                        Resettable::from(Option::from(state.anchor)),
                        Some((state.deposit, registration)),
                        last_interaction,
                    ),
                )
            }),
            cc_members: iter::empty(),
            proposals: iter::empty(),
        },
        Default::default(),
        iter::empty(),
        BTreeSet::new(),
    )?;
    transaction.commit()
}

fn import_proposals(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    proposals: Vec<ProposalState>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_proposals(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    info!(what = "proposals", size = proposals.len());

    transaction.save(
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: proposals
                .into_iter()
                .map(|proposal| -> Result<_, Box<dyn std::error::Error>> {
                    let proposal_index = proposal.id.action_index as usize;
                    Ok((
                        proposal.id,
                        proposals::Value {
                            proposed_in: ProposalPointer {
                                transaction: TransactionPointer {
                                    slot: era_history.epoch_bounds(proposal.proposed_in)?.start,
                                    transaction_index: 0,
                                },
                                proposal_index,
                            },
                            valid_until: proposal.proposed_in
                                + protocol_parameters.gov_action_lifetime as u64,
                            proposal: proposal.procedure,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter(),
        },
        Default::default(),
        iter::empty(),
        BTreeSet::new(),
    )?;
    transaction.commit()?;

    Ok(())
}

fn import_stake_pools(
    db: &impl Store,
    point: &Point,
    epoch: Epoch,
    pools: BTreeMap<PoolId, PoolParams>,
    updates: BTreeMap<PoolId, PoolParams>,
    retirements: BTreeMap<PoolId, Epoch>,
) -> Result<(), impl std::error::Error> {
    let mut state = amaru_ledger::state::diff_epoch_reg::DiffEpochReg::default();
    for (pool, params) in pools.into_iter() {
        state.register(pool, params);
    }

    for (pool, params) in updates.into_iter() {
        state.register(pool, params);
    }

    for (pool, epoch) in retirements.into_iter() {
        state.unregister(pool, epoch);
    }

    info!(
        what = "stake_pools",
        registered = state.registered.len(),
        retiring = state.unregistered.len(),
    );
    let transaction = db.create_transaction();
    transaction.with_pools(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;
    transaction.commit()?;

    let transaction = db.create_transaction();
    transaction.save(
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: state
                .registered
                .into_iter()
                .flat_map(move |(_, registrations)| {
                    registrations
                        .into_iter()
                        .map(|r| (r, epoch))
                        .collect::<Vec<_>>()
                }),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: iter::empty(),
        },
        store::Columns {
            pools: state.unregistered.into_iter(),
            utxo: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: iter::empty(),
        },
        iter::empty(),
        BTreeSet::new(),
    )?;
    transaction.commit()
}

fn import_pots(
    db: &impl Store,
    treasury: u64,
    reserves: u64,
    fees: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_pots(|mut row| {
        let pots = row.borrow_mut();
        pots.treasury = treasury;
        pots.reserves = reserves;
        pots.fees = fees;
    })?;
    transaction.commit()?;
    info!(what = "pots", treasury, reserves, fees);
    Ok(())
}

fn import_accounts(
    db: &impl Store,
    point: &Point,
    accounts: BTreeMap<StakeCredential, Account>,
    rewards_updates: &mut BTreeMap<StakeCredential, Set<Reward>>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_accounts(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    let mut credentials = accounts
        .into_iter()
        .map(
            |(
                credential,
                Account {
                    rewards_and_deposit,
                    pool,
                    drep,
                    ..
                },
            )| {
                let (rewards, deposit) = Option::<(Lovelace, Lovelace)>::from(rewards_and_deposit)
                    .unwrap_or((0, protocol_parameters.stake_credential_deposit));

                let rewards_update = match rewards_updates.remove(&credential) {
                    None => 0,
                    Some(set) => set.iter().fold(0, |total, update| total + update.amount),
                };

                (
                    credential,
                    (
                        Resettable::from(Option::<PoolId>::from(pool)),
                        //No slot to retrieve. All registrations coming from snapshot are considered valid.
                        Resettable::from(
                            Option::<DRep>::from(drep)
                                .map(|drep| (drep, *DEFAULT_CERTIFICATE_POINTER)),
                        ),
                        Some(deposit),
                        rewards + rewards_update,
                    ),
                )
            },
        )
        .collect::<Vec<_>>();

    info!(what = "credentials", size = credentials.len());

    let progress = ProgressBar::new(credentials.len() as u64).with_style(
        ProgressStyle::with_template("  Accounts {bar:70} {pos:>7}/{len:7}")?,
    );

    while !credentials.is_empty() {
        let n = std::cmp::min(BATCH_SIZE, credentials.len());
        let chunk = credentials.drain(0..n);

        transaction.save(
            point,
            None,
            store::Columns {
                utxo: iter::empty(),
                pools: iter::empty(),
                accounts: chunk,
                dreps: iter::empty(),
                cc_members: iter::empty(),
                proposals: iter::empty(),
            },
            Default::default(),
            iter::empty(),
            BTreeSet::new(),
        )?;

        progress.inc(n as u64);
    }

    transaction.commit()?;
    progress.finish_and_clear();

    Ok(())
}

#[derive(Debug)]
enum StrictMaybe<T> {
    Nothing,
    Just(T),
}

impl<'b, C, T: cbor::decode::Decode<'b, C>> cbor::decode::Decode<'b, C> for StrictMaybe<T> {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        if len != Some(0) {
            let t = d.decode_with(ctx)?;
            Ok(StrictMaybe::Just(t))
        } else {
            Ok(StrictMaybe::Nothing)
        }
    }
}

impl<T> From<StrictMaybe<T>> for Option<T> {
    fn from(value: StrictMaybe<T>) -> Option<T> {
        match value {
            StrictMaybe::Nothing => None,
            StrictMaybe::Just(t) => Some(t),
        }
    }
}

#[derive(Debug)]
struct Reward {
    #[allow(dead_code)]
    kind: RewardKind,
    #[allow(dead_code)]
    pool: PoolId,
    amount: Lovelace,
}

impl<'b, C> cbor::decode::Decode<'b, C> for Reward {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let kind = d.decode_with(ctx)?;
        let pool = d.decode_with(ctx)?;
        let amount = d.decode_with(ctx)?;
        Ok(Reward { kind, pool, amount })
    }
}

#[derive(Debug)]
enum RewardKind {
    Member,
    Leader,
}

impl<'b, C> cbor::decode::Decode<'b, C> for RewardKind {
    #[allow(clippy::panic)]
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(match d.u8()? {
            0 => RewardKind::Member,
            1 => RewardKind::Leader,
            k => panic!("unexpected reward kind: {k}"),
        })
    }
}

#[derive(Debug)]
struct Account {
    rewards_and_deposit: StrictMaybe<(Lovelace, Lovelace)>,
    #[allow(dead_code)]
    pointers: Set<(u64, u64, u64)>,
    pool: StrictMaybe<PoolId>,
    drep: StrictMaybe<DRep>,
}

impl<'b, C> cbor::decode::Decode<'b, C> for Account {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Account {
            rewards_and_deposit: d.decode_with(ctx)?,
            pointers: d.decode_with(ctx)?,
            pool: d.decode_with(ctx)?,
            drep: d.decode_with(ctx)?,
        })
    }
}

#[derive(Debug)]
struct DRepState {
    expiry: Epoch,
    anchor: StrictMaybe<Anchor>,
    deposit: Lovelace,
    #[allow(dead_code)]
    delegators: Set<StakeCredential>,
}

impl<'b, C> cbor::decode::Decode<'b, C> for DRepState {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(DRepState {
            expiry: d.decode_with(ctx)?,
            anchor: d.decode_with(ctx)?,
            deposit: d.decode_with(ctx)?,
            delegators: d.decode_with(ctx)?,
        })
    }
}

#[derive(Debug)]
struct ProposalState {
    id: ProposalId,
    procedure: Proposal,
    proposed_in: Epoch,
    #[allow(dead_code)]
    expires_after: Epoch,
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProposalState {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let id = d.decode_with(ctx)?;
        d.skip()?; // CC Votes
        d.skip()?; // DRep Votes
        d.skip()?; // SPO Votes
        let procedure = d.decode_with(ctx)?;
        let proposed_in = d.decode_with(ctx)?;
        let expires_after = d.decode_with(ctx)?;

        Ok(ProposalState {
            id,
            procedure,
            proposed_in,
            expires_after,
        })
    }
}
