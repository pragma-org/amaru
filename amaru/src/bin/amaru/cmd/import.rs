use amaru::ledger::{
    self,
    kernel::{
        epoch_from_slot, Epoch, Point, PoolId, PoolParams, TransactionInput, TransactionOutput,
    },
    store::{
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
                    // NOTE: We are importing pools for the next epoch onwards, so any pool update
                    // has technically become active at the epoch boundary. So it is sufficient to
                    // import only the updates if any.
                    if !updates.contains_key(&pool) {
                        state.register(pool, params);
                    }
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
                    },
                    store::Columns {
                        pools: state.unregistered.into_iter(),
                        utxo: iter::empty(),
                    },
                )
                .into_diagnostic()?;
            }

            // Epoch State / Ledger State / Cert State / Delegation state
            d.skip().into_diagnostic()?;

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

    db.next_snapshot(epoch_from_slot(point.slot_or_default()))
        .into_diagnostic()?;

    Ok(())
}
