use amaru::ledger::{
    self,
    kernel::{Point, TransactionInput, TransactionOutput},
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
    ///   61841373.fddcbaddb5fce04f01d26a39d5bab6b59ed2387412e9cf7431f00f25f9ce557d
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
    let _epoch_state_len = d.array().into_diagnostic()?;

    // Epoch State / Account State

    d.skip().into_diagnostic()?;

    // Epoch State / Ledger State
    let _ledger_state_len = d.array().into_diagnostic()?;

    // Epoch State / Ledger State / Cert State
    d.skip().into_diagnostic()?;

    // Epoch State / Ledger State / UTxO State
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
                    as Box<(dyn Iterator<Item = (TransactionInput, TransactionOutput)>)>,
                ..Default::default()
            },
            Default::default(),
        )
        .into_diagnostic()?;

        progress.inc(n as u64);
    }

    db.save(&point, Default::default(), Default::default())
        .into_diagnostic()?;

    progress.finish();

    Ok(())
}
