use amaru::ledger::{
    self,
    kernel::{
        epoch_from_slot, DRep, Epoch, Lovelace, Point, PoolId, PoolParams, Set, StakeCredential,
        TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
    },
    store::{
        columns::pools,
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
    #[arg(long)]
    snapshot: PathBuf,

    /// Path to the ledger database folder.
    #[arg(long)]
    out: PathBuf,

    /// A `slot#block_header_hash` snapshot's point.
    ///
    /// For example:
    ///   68774372.36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4
    #[arg(long, verbatim_doc_comment)]
    date: String,
}

#[derive(Debug, thiserror::Error, Diagnostic)]
enum Error<'a> {
    #[error("malformed date: {}", .0)]
    MalformedDate(&'a str),
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

pub async fn run(args: Args) -> miette::Result<()> {
    let point = super::parse_point(&args.date, Error::MalformedDate).into_diagnostic()?;

    fs::create_dir_all(&args.out).into_diagnostic()?;
    let mut db = ledger::store::rocksdb::RocksDB::empty(&args.out).into_diagnostic()?;

    let bytes = fs::read(&args.snapshot).into_diagnostic()?;
    let mut d = cbor::Decoder::new(&bytes);

    // Epoch State
    {
        let _epoch_state_len = d.array().into_diagnostic()?;

        // Epoch State / Account State
        d.skip().into_diagnostic()?;

        // Epoch State / Ledger State
        let _ledger_state_len = d.array().into_diagnostic()?;

        // Epoch State / Ledger State / Cert State
        {
            let _cert_state_len = d.array().into_diagnostic()?;

            // Epoch State / Ledger State / Cert State / Voting State
            d.skip().into_diagnostic()?;

            // Epoch State / Ledger State / Cert State / Pool State
            {
                let _pool_state_len = d.array().into_diagnostic()?;

                let pools: HashMap<PoolId, PoolParams> = d.decode().into_diagnostic()?;

                let updates: HashMap<PoolId, PoolParams> = d.decode().into_diagnostic()?;

                let retirements: HashMap<PoolId, Epoch> = d.decode().into_diagnostic()?;

                // Deposits
                d.skip().into_diagnostic()?;

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

                let current_epoch = epoch_from_slot(point.slot_or_default());

                db.save(
                    &Point::Origin,
                    store::Columns {
                        utxo: iter::empty(),
                        pools: state
                            .registered
                            .into_iter()
                            .flat_map(move |(_, registrations)| {
                                registrations
                                    .into_iter()
                                    .map(|r| (r, current_epoch))
                                    .collect::<Vec<_>>()
                            }),
                        accounts: iter::empty(),
                    },
                    store::Columns {
                        pools: state.unregistered.into_iter(),
                        utxo: iter::empty(),
                        accounts: iter::empty(),
                    },
                )
                .into_diagnostic()?;
            }

            // Epoch State / Ledger State / Cert State / Delegation state
            {
                let _delegation_state_len = d.array().into_diagnostic()?;

                // Epoch State / Ledger State / Cert State / Delegation state / dsUnified
                {
                    let _stake_credentials_len = d.array().into_diagnostic()?;

                    // credentials
                    {
                        let mut credentials =
                            d.decode::<HashMap<
                                StakeCredential,
                                (
                                    StrictMaybe<(Lovelace, Lovelace)>,
                                    Set<()>,
                                    StrictMaybe<PoolId>,
                                    StrictMaybe<DRep>,
                                ),
                            >>()
                            .into_diagnostic()?
                            .into_iter()
                            .map(
                                |(credential, (rewards_and_deposit, _pointers, pool, _drep))| {
                                    let (rewards, deposit) =
                                        Option::<(Lovelace, Lovelace)>::from(rewards_and_deposit)
                                            .unwrap_or((0, STAKE_CREDENTIAL_DEPOSIT as u64));

                                    (
                                        credential,
                                        Option::<PoolId>::from(pool),
                                        Some(deposit),
                                        rewards,
                                    )
                                },
                            )
                            .collect::<Vec<(
                                StakeCredential,
                                Option<PoolId>,
                                Option<Lovelace>,
                                Lovelace,
                            )>>();

                        info!(what = "credentials", size = credentials.len());

                        let progress = ProgressBar::new(credentials.len() as u64).with_style(
                            ProgressStyle::with_template("  Accounts {bar:70} {pos:>7}/{len:7}")
                                .unwrap(),
                        );

                        while !credentials.is_empty() {
                            let n = std::cmp::min(BATCH_SIZE, credentials.len());
                            let chunk = credentials.drain(0..n);

                            db.save(
                                &Point::Origin,
                                store::Columns {
                                    utxo: iter::empty(),
                                    pools: iter::empty(),
                                    accounts: chunk,
                                },
                                Default::default(),
                            )
                            .into_diagnostic()?;

                            progress.inc(n as u64);
                        }

                        progress.finish();
                    }

                    // pointers
                    {
                        d.skip().into_diagnostic()?;
                    }
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
                let _utxo_state_len = d.array().into_diagnostic()?;

                let mut utxo: Vec<(TransactionInput, TransactionOutput)> = d
                    .decode::<HashMap<TransactionInput, TransactionOutput>>()
                    .into_diagnostic()?
                    .into_iter()
                    .collect::<Vec<(TransactionInput, TransactionOutput)>>();

                info!(what = "utxo_entries", size = utxo.len());

                let progress = ProgressBar::new(utxo.len() as u64).with_style(
                    ProgressStyle::with_template("  UTxO entries {bar:70} {pos:>7}/{len:7}")
                        .unwrap(),
                );

                while !utxo.is_empty() {
                    let n = std::cmp::min(BATCH_SIZE, utxo.len());
                    let chunk = utxo.drain(0..n);

                    db.save(
                        &Point::Origin,
                        store::Columns {
                            utxo: chunk,
                            pools: iter::empty(),
                            accounts: iter::empty(),
                        },
                        Default::default(),
                    )
                    .into_diagnostic()?;

                    progress.inc(n as u64);
                }

                progress.finish();
            }
        }
    }

    db.save(&point, Default::default(), Default::default())
        .into_diagnostic()?;

    let epoch = epoch_from_slot(point.slot_or_default());

    db.next_snapshot(epoch).into_diagnostic()?;

    db.with_pools(|iterator| {
        for (_, pool) in iterator {
            pools::Row::tick(pool, epoch + 1)
        }
    })
    .into_diagnostic()?;

    Ok(())
}
