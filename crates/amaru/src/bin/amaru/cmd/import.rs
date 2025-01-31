use amaru::ledger::{
    self,
    kernel::{
        epoch_from_slot, DRep, Epoch, Lovelace, Point, PoolId, PoolParams, Set, StakeCredential,
        TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
    },
    store::{
        columns::{pools, pots, slots},
        Store, {self},
    },
};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use miette::{Diagnostic, IntoDiagnostic};
use pallas_codec::minicbor as cbor;
use std::{collections::HashMap, fs, iter, path::PathBuf};
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
    #[arg(long, verbatim_doc_comment)]
    snapshot: PathBuf,

    /// Path of the ledger on-disk storage.
    #[arg(long, default_value = super::DEFAULT_LEDGER_DB_DIR)]
    ledger_dir: PathBuf,
}

#[derive(Debug, thiserror::Error, Diagnostic)]
enum Error<'a> {
    #[error("malformed date: {}", .0)]
    MalformedDate(&'a str),
}

pub async fn run(args: Args) -> miette::Result<()> {
    let point = super::parse_point(
        args.snapshot
            .as_path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap(),
        Error::MalformedDate,
    )
    .into_diagnostic()?;

    fs::create_dir_all(&args.ledger_dir).into_diagnostic()?;
    let mut db = ledger::store::rocksdb::RocksDB::empty(&args.ledger_dir).into_diagnostic()?;
    let bytes = fs::read(&args.snapshot).into_diagnostic()?;

    let epoch = decode_new_epoch_state(&db, &bytes, &point)?;

    db.save(
        &point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
    )
    .into_diagnostic()?;

    db.next_snapshot(epoch, None).into_diagnostic()?;

    db.with_pools(|iterator| {
        for (_, pool) in iterator {
            pools::Row::tick(pool, epoch + 1)
        }
    })
    .into_diagnostic()?;
    info!("Imported snapshot for epoch {}", epoch);
    Ok(())
}

fn decode_new_epoch_state(
    db: &store::rocksdb::RocksDB,
    bytes: &[u8],
    point: &Point,
) -> miette::Result<Epoch> {
    let mut d = cbor::Decoder::new(bytes);

    d.array().into_diagnostic()?;

    // EpochNo
    let epoch = d.u64().into_diagnostic()?;
    assert_eq!(epoch, epoch_from_slot(point.slot_or_default()));
    info!(epoch, "importing_snapshot");

    // Previous blocks made
    d.skip().into_diagnostic()?;

    // Current blocks made
    // NOTE: We use the current blocks made here as we assume that users are providing snapshots of
    // the last block of the epoch. We have no intrinsic ways to check that this is the case since
    // we do not know what the last block of an epoch is, and we can't reliably look at the number
    // of blocks either.
    import_block_issuers(db, d.decode().into_diagnostic()?)?;

    let accounts: HashMap<StakeCredential, Account>;
    let fees: i64;
    let treasury: i64;
    let reserves: i64;

    // Epoch State
    {
        d.array().into_diagnostic()?;

        // Epoch State / Account State
        d.array().into_diagnostic()?;
        treasury = d.decode().into_diagnostic()?;
        reserves = d.decode().into_diagnostic()?;

        // Epoch State / Ledger State
        d.array().into_diagnostic()?;

        // Epoch State / Ledger State / Cert State
        {
            d.array().into_diagnostic()?;

            // Epoch State / Ledger State / Cert State / Voting State
            d.skip().into_diagnostic()?;

            // Epoch State / Ledger State / Cert State / Pool State
            {
                d.array().into_diagnostic()?;
                import_stake_pools(
                    db,
                    epoch,
                    // Pools
                    d.decode().into_diagnostic()?,
                    // Updates
                    d.decode().into_diagnostic()?,
                    // Retirements
                    d.decode().into_diagnostic()?,
                )?;
                // Deposits
                d.skip().into_diagnostic()?;
            }

            // Epoch State / Ledger State / Cert State / Delegation state
            {
                d.array().into_diagnostic()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsUnified
                {
                    d.array().into_diagnostic()?;
                    // credentials
                    accounts = d.decode().into_diagnostic()?;
                    // pointers
                    d.skip().into_diagnostic()?;
                }

                // Epoch State / Ledger State / Cert State / Delegation state / dsFutureGenDelegs
                d.skip().into_diagnostic()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsGenDelegs
                d.skip().into_diagnostic()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsIRewards
                d.skip().into_diagnostic()?;
            }

            // Epoch State / Ledger State / UTxO State
            {
                d.array().into_diagnostic()?;

                import_utxo(
                    db,
                    d.decode::<HashMap<TransactionInput, TransactionOutput>>()
                        .into_diagnostic()?
                        .into_iter()
                        .collect::<Vec<(TransactionInput, TransactionOutput)>>(),
                )?;

                let _deposited: u64 = d.decode().into_diagnostic()?;

                fees = d.decode().into_diagnostic()?;

                // Epoch State / Ledger State / UTxO State / utxosGovState
                d.skip().into_diagnostic()?;

                // Epoch State / Ledger State / UTxO State / utxosStakeDistr
                d.skip().into_diagnostic()?;

                // Epoch State / Ledger State / UTxO State / utxosDonation
                d.skip().into_diagnostic()?;
            }
        }

        // Epoch State / Snapshots
        d.skip().into_diagnostic()?;
        // Epoch State / NonMyopic
        d.skip().into_diagnostic()?;
    }

    // Rewards Update
    {
        d.array().into_diagnostic()?;
        d.array().into_diagnostic()?;
        assert_eq!(
            d.u32().into_diagnostic()?,
            1,
            "expected complete pulsing reward state"
        );
        d.array().into_diagnostic()?;

        let delta_treasury: i64 = d.decode().into_diagnostic()?;

        let delta_reserves: i64 = d.decode().into_diagnostic()?;

        let mut rewards: HashMap<StakeCredential, Set<Reward>> = d.decode().into_diagnostic()?;
        let delta_fees: i64 = d.decode().into_diagnostic()?;

        // NonMyopic
        d.skip().into_diagnostic()?;

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
    db: &store::rocksdb::RocksDB,
    blocks: HashMap<PoolId, u64>,
) -> miette::Result<()> {
    let batch = db.unsafe_transaction();
    db.with_block_issuers(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })
    .into_diagnostic()?;

    let mut fake_slot = 0;
    for (pool, mut count) in blocks.into_iter() {
        while count > 0 {
            slots::rocksdb::put(&batch, &fake_slot, slots::Row::new(pool)).into_diagnostic()?;
            count -= 1;
            fake_slot += 1;
        }
    }
    info!(what = "block_issuers", count = fake_slot);
    batch.commit().into_diagnostic()
}

fn import_utxo(
    db: &store::rocksdb::RocksDB,
    mut utxo: Vec<(TransactionInput, TransactionOutput)>,
) -> miette::Result<()> {
    info!(what = "utxo_entries", size = utxo.len());

    let progress_delete = ProgressBar::no_length().with_style(
        ProgressStyle::with_template("  Pruning UTxO entries {spinner} {elapsed}").unwrap(),
    );

    db.with_utxo(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
            progress_delete.tick();
        }
    })
    .into_diagnostic()?;

    progress_delete.finish_and_clear();

    let progress = ProgressBar::new(utxo.len() as u64).with_style(
        ProgressStyle::with_template("  UTxO entries {bar:70} {pos:>7}/{len:7}").unwrap(),
    );

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
            },
            Default::default(),
            iter::empty(),
        )
        .into_diagnostic()?;

        progress.inc(n as u64);
    }

    progress.finish_and_clear();

    Ok(())
}

fn import_stake_pools(
    db: &store::rocksdb::RocksDB,
    epoch: Epoch,
    pools: HashMap<PoolId, PoolParams>,
    updates: HashMap<PoolId, PoolParams>,
    retirements: HashMap<PoolId, Epoch>,
) -> miette::Result<()> {
    let mut state = ledger::state::diff_epoch_reg::DiffEpochReg::default();
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
    })
    .into_diagnostic()?;

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
        },
        store::Columns {
            pools: state.unregistered.into_iter(),
            utxo: iter::empty(),
            accounts: iter::empty(),
        },
        iter::empty(),
    )
    .into_diagnostic()
}

fn import_pots(
    db: &store::rocksdb::RocksDB,
    treasury: u64,
    reserves: u64,
    fees: u64,
) -> miette::Result<()> {
    let batch = db.unsafe_transaction();
    pots::rocksdb::put(&batch, pots::Row::new(treasury, reserves, fees)).into_diagnostic()?;
    batch.commit().into_diagnostic()?;
    info!(what = "pots", treasury, reserves, fees);
    Ok(())
}

fn import_accounts(
    db: &store::rocksdb::RocksDB,
    accounts: HashMap<StakeCredential, Account>,
    rewards_updates: &mut HashMap<StakeCredential, Set<Reward>>,
) -> miette::Result<()> {
    db.with_accounts(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })
    .into_diagnostic()?;

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
                        Option::<PoolId>::from(pool),
                        Some(deposit),
                        rewards + rewards_update,
                    ),
                )
            },
        )
        .collect::<Vec<_>>();

    info!(what = "credentials", size = credentials.len());

    let progress = ProgressBar::new(credentials.len() as u64)
        .with_style(ProgressStyle::with_template("  Accounts {bar:70} {pos:>7}/{len:7}").unwrap());

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
            },
            Default::default(),
            iter::empty(),
        )
        .into_diagnostic()?;

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
