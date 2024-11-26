use amaru::ledger::{
    self,
    kernel::{epoch_slot, Epoch, Point, PoolId, PoolParams, TransactionInput, TransactionOutput},
    store::{self, Store},
};
use clap::Parser;
use indicatif::ProgressBar;
use miette::{Diagnostic, IntoDiagnostic};
use pallas_codec::minicbor as cbor;
use std::{collections::HashMap, fs, path::PathBuf};

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

    let db = ledger::store::impl_rocksdb::RocksDB::new(&args.out).into_diagnostic()?;

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

                let mut state = ledger::state::DiffEpochReg::default();
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

                eprintln!(
                    "importing {} Stake Pools entries...",
                    state.registered.len() + state.unregistered.len()
                );

                let current_epoch = epoch_slot(point.slot_or_default());

                db.save(
                    &Point::Origin,
                    store::Add {
                        pools: Box::new(state.registered.into_iter().flat_map(
                            move |(_, registrations)| {
                                registrations
                                    .into_iter()
                                    .map(|r| (r, current_epoch))
                                    .collect::<Vec<_>>()
                            },
                        )),
                        ..Default::default()
                    },
                    store::Remove {
                        pools: Box::new(state.unregistered.into_iter()),
                        ..Default::default()
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

                eprintln!("importing {} UTxO entries...", utxo.len());

                let progress = ProgressBar::new(utxo.len() as u64);

                while !utxo.is_empty() {
                    let n = std::cmp::min(BATCH_SIZE, utxo.len());
                    let chunk = utxo.drain(0..n);

                    db.save(
                        &Point::Origin,
                        store::Add {
                            utxo: Box::new(chunk)
                                as Box<
                                    (dyn Iterator<Item = (TransactionInput, TransactionOutput)>),
                                >,
                            ..Default::default()
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

    Ok(())
}
