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
    epoch_from_slot, DRep, Epoch, Lovelace, Point, PoolId, PoolParams, Set, StakeCredential,
    TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
};
use amaru_ledger::{
    self,
    store::{
        Store, {self},
    },
};
use amaru_stores::rocksdb::{columns::*, RocksDB};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use pallas_codec::minicbor as cbor;
use std::{
    collections::{BTreeSet, HashMap},
    fs, iter,
    path::PathBuf,
};
use tracing::info;

const BATCH_SIZE: usize = 5000;

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
    #[arg(long, value_name = "DIR", default_value = super::DEFAULT_LEDGER_DB_DIR)]
    ledger_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("malformed date: {}", .0)]
    MalformedDate(String),
    #[error("You must provide either a single .cbor snapshot file (--snapshot) or a directory containing multiple .cbor snapshots (--snapshot-dir)")]
    IncorrectUsage,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if !args.snapshot.is_empty() {
        import_all(&args.snapshot, &args.ledger_dir).await
    } else if let Some(snapshot_dir) = args.snapshot_dir {
        let mut snapshots = fs::read_dir(snapshot_dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("cbor"))
            .collect::<Vec<_>>();
        snapshots.sort();

        import_all(&snapshots, &args.ledger_dir).await
    } else {
        Err(Error::IncorrectUsage.into())
    }
}

async fn import_all(
    snapshots: &Vec<PathBuf>,
    ledger_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing {} snapshots", snapshots.len());
    for snapshot in snapshots {
        import_one(snapshot, ledger_dir).await?;
    }
    Ok(())
}

#[allow(clippy::unwrap_used)]
async fn import_one(
    snapshot: &PathBuf,
    ledger_dir: &PathBuf,
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
    let mut db = amaru_stores::rocksdb::RocksDB::empty(ledger_dir)?;
    let bytes = fs::read(snapshot)?;

    let epoch = decode_new_epoch_state(&db, &bytes, &point)?;

    db.save(
        &point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
        BTreeSet::new(),
    )?;

    db.next_snapshot(epoch, None)?;

    db.with_pools(|iterator| {
        for (_, pool) in iterator {
            amaru_ledger::store::columns::pools::Row::tick(pool, epoch + 1)
        }
    })?;
    info!("Imported snapshot for epoch {}", epoch);
    Ok(())
}

fn decode_new_epoch_state(
    db: &RocksDB,
    bytes: &[u8],
    point: &Point,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut d = cbor::Decoder::new(bytes);

    d.array()?;

    // EpochNo
    let epoch = d.u64()?;
    assert_eq!(epoch, epoch_from_slot(point.slot_or_default()));
    info!(epoch, "importing_snapshot");

    // Previous blocks made
    d.skip()?;

    // Current blocks made
    // NOTE: We use the current blocks made here as we assume that users are providing snapshots of
    // the last block of the epoch. We have no intrinsic ways to check that this is the case since
    // we do not know what the last block of an epoch is, and we can't reliably look at the number
    // of blocks either.
    import_block_issuers(db, d.decode()?)?;

    let accounts: HashMap<StakeCredential, Account>;
    let fees: i64;
    let treasury: i64;
    let reserves: i64;

    // Epoch State
    {
        d.array()?;

        // Epoch State / Account State
        d.array()?;
        treasury = d.decode()?;
        reserves = d.decode()?;

        // Epoch State / Ledger State
        d.array()?;

        // Epoch State / Ledger State / Cert State
        {
            d.array()?;

            // Epoch State / Ledger State / Cert State / Voting State
            d.skip()?;

            // Epoch State / Ledger State / Cert State / Pool State
            {
                d.array()?;
                import_stake_pools(
                    db,
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
            }

            // Epoch State / Ledger State / Cert State / Delegation state
            {
                d.array()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsUnified
                {
                    d.array()?;
                    // credentials
                    accounts = d.decode()?;
                    // pointers
                    d.skip()?;
                }

                // Epoch State / Ledger State / Cert State / Delegation state / dsFutureGenDelegs
                d.skip()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsGenDelegs
                d.skip()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsIRewards
                d.skip()?;
            }

            // Epoch State / Ledger State / UTxO State
            {
                d.array()?;

                import_utxo(
                    db,
                    d.decode::<HashMap<TransactionInput, TransactionOutput>>()?
                        .into_iter()
                        .collect::<Vec<(TransactionInput, TransactionOutput)>>(),
                )?;

                let _deposited: u64 = d.decode()?;

                fees = d.decode()?;

                // Epoch State / Ledger State / UTxO State / utxosGovState
                d.skip()?;

                // Epoch State / Ledger State / UTxO State / utxosStakeDistr
                d.skip()?;

                // Epoch State / Ledger State / UTxO State / utxosDonation
                d.skip()?;
            }
        }

        // Epoch State / Snapshots
        d.skip()?;
        // Epoch State / NonMyopic
        d.skip()?;
    }

    // Rewards Update
    {
        d.array()?;
        d.array()?;
        assert_eq!(d.u32()?, 1, "expected complete pulsing reward state");
        d.array()?;

        let delta_treasury: i64 = d.decode()?;

        let delta_reserves: i64 = d.decode()?;

        let mut rewards: HashMap<StakeCredential, Set<Reward>> = d.decode()?;
        let delta_fees: i64 = d.decode()?;

        // NonMyopic
        d.skip()?;

        import_accounts(db, accounts, &mut rewards)?;

        let unclaimed_rewards = rewards.into_iter().fold(0, |total, (_, rewards)| {
            total + rewards.into_iter().fold(0, |inner, r| inner + r.amount)
        });

        import_pots(
            db,
            (treasury + delta_treasury) as u64 + unclaimed_rewards,
            (reserves - delta_reserves) as u64,
            (fees - delta_fees) as u64,
        )?;
    }

    Ok(epoch)
}

fn import_block_issuers(
    db: &RocksDB,
    blocks: HashMap<PoolId, u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let batch = db.unsafe_transaction();
    db.with_block_issuers(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    let mut fake_slot = 0;
    for (pool, mut count) in blocks.into_iter() {
        while count > 0 {
            slots::put(
                &batch,
                &fake_slot,
                amaru_ledger::store::columns::slots::Row::new(pool),
            )?;
            count -= 1;
            fake_slot += 1;
        }
    }
    info!(what = "block_issuers", count = fake_slot);
    batch.commit().map_err(Into::into)
}

fn import_utxo(
    db: &impl Store,
    mut utxo: Vec<(TransactionInput, TransactionOutput)>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(what = "utxo_entries", size = utxo.len());

    let progress_delete = ProgressBar::no_length().with_style(ProgressStyle::with_template(
        "  Pruning UTxO entries {spinner} {elapsed}",
    )?);

    db.with_utxo(|iterator| {
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

        db.save(
            &Point::Origin,
            None,
            store::Columns {
                utxo: chunk,
                pools: iter::empty(),
                accounts: iter::empty(),
                dreps: iter::empty(),
            },
            Default::default(),
            iter::empty(),
            BTreeSet::new(),
        )?;

        progress.inc(n as u64);
    }

    progress.finish_and_clear();

    Ok(())
}

fn import_stake_pools(
    db: &impl Store,
    epoch: Epoch,
    pools: HashMap<PoolId, PoolParams>,
    updates: HashMap<PoolId, PoolParams>,
    retirements: HashMap<PoolId, Epoch>,
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

    db.with_pools(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    db.save(
        &Point::Origin,
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
        },
        store::Columns {
            pools: state.unregistered.into_iter(),
            utxo: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
        },
        iter::empty(),
        BTreeSet::new(),
    )
}

fn import_pots(
    db: &RocksDB,
    treasury: u64,
    reserves: u64,
    fees: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let batch = db.unsafe_transaction();
    pots::put(
        &batch,
        amaru_ledger::store::columns::pots::Row::new(treasury, reserves, fees),
    )?;
    batch.commit()?;
    info!(what = "pots", treasury, reserves, fees);
    Ok(())
}

fn import_accounts(
    db: &impl Store,
    accounts: HashMap<StakeCredential, Account>,
    rewards_updates: &mut HashMap<StakeCredential, Set<Reward>>,
) -> Result<(), Box<dyn std::error::Error>> {
    db.with_accounts(|iterator| {
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
                    ..
                },
            )| {
                let (rewards, deposit) = Option::<(Lovelace, Lovelace)>::from(rewards_and_deposit)
                    .unwrap_or((0, STAKE_CREDENTIAL_DEPOSIT as u64));

                let rewards_update = match rewards_updates.remove(&credential) {
                    None => 0,
                    Some(set) => set.iter().fold(0, |total, update| total + update.amount),
                };

                (
                    credential,
                    (
                        (Option::<PoolId>::from(pool), None),
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

        db.save(
            &Point::Origin,
            None,
            store::Columns {
                utxo: iter::empty(),
                pools: iter::empty(),
                accounts: chunk,
                dreps: iter::empty(),
            },
            Default::default(),
            iter::empty(),
            BTreeSet::new(),
        )?;

        progress.inc(n as u64);
    }

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
    #[allow(dead_code)]
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
