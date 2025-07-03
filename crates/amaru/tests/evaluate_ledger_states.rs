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
    cbor, network::NetworkName, AuxiliaryData, EraHistory, Hasher, KeepRaw, MintedTransactionBody,
    MintedTx, MintedWitnessSet, TransactionPointer,
};
use amaru_ledger::{
    self,
    context::DefaultValidationContext,
    rules::transaction,
    state::{self},
    store::{EpochTransitionProgress, ReadOnlyStore, Store, TransactionalContext},
};
use amaru_stores::rocksdb::RocksDB;
use pallas_codec::utils::AnyCbor;
use std::{collections::BTreeSet, env, fs, iter, ops::Deref, path::PathBuf};

struct TestContext {
    ledger_dir: PathBuf,
    pparams_dir: PathBuf,
    snapshots: PathBuf,
}

fn get_test_context() -> Result<TestContext, Box<dyn std::error::Error>> {
    let working_dir = env::current_dir()?;
    Ok(TestContext {
        ledger_dir: working_dir.join(PathBuf::from("../../ledger.db")),
        pparams_dir: working_dir.join(PathBuf::from("../../cardano-blueprint/src/ledger/conformance-test-vectors/eras/conway/impl/dump/pparams-by-hash/")),
        snapshots: working_dir.join(PathBuf::from("../../cardano-blueprint/src/ledger/conformance-test-vectors/eras/conway/impl/dump/")),
    })
}

#[test]
fn evaluate_vector() -> Result<(), Box<dyn std::error::Error>> {
    let network = NetworkName::Testnet(1);
    let era_history = network.into();
    match get_test_context() {
        Ok(tc) => {
            for snapshot in fs::read_dir(tc.snapshots)? {
                let snapshot = snapshot?;
                if snapshot.file_name() == "pparams-by-hash" {
                    continue;
                }
                for vector in fs::read_dir(snapshot.path())? {
                    let vector = vector?;
                    let vector_file = fs::read(vector.path())?;
                    let record: NESVector = cbor::decode(&vector_file)?;
                    let () = import_vector(record, &tc.ledger_dir, era_history, &tc.pparams_dir)?;
                }
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                "skipping vector evaluation because some env vars are missing: {}",
                e
            );
            Ok(())
        }
    }
}

fn peek_epoch(nes: &AnyCbor) -> Result<u64, Box<dyn std::error::Error>> {
    let mut d = cbor::decode::Decoder::new(nes.raw_bytes());
    let _ = d.array()?;
    Ok(d.u64()?)
}

#[derive(cbor::Decode)]
#[allow(dead_code)]
struct NESVector {
    #[n(0)]
    new_nes: Vec<AnyCbor>,
    #[n(1)]
    old_nes: AnyCbor,
    #[n(2)]
    cbor: AnyCbor,
    #[n(3)]
    success: bool,
    #[n(4)]
    test_state: String,
}

fn import_vector(
    record: NESVector,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
    pparams_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let epoch = peek_epoch(&record.old_nes)?;
    let point = amaru_kernel::parse_point(&format!(
        "{}.0000000000000000000000000000000000000000000000000000000000000000",
        epoch * 432000
    ))?;

    fs::create_dir_all(ledger_dir)?;
    let db = RocksDB::empty(ledger_dir, era_history)?;

    let epoch = amaru_ledger::state::snapshot::decode_new_epoch_state(
        &db,
        record.old_nes.raw_bytes(),
        &point,
        era_history,
        amaru_ledger::state::snapshot::no_progress_bar,
        Some(pparams_dir),
        false,
    )?;
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

    let snapshot = db.snapshots()?.last().map(|s| *s + 1).unwrap_or(epoch);
    db.next_snapshot(snapshot)?;

    let transaction = db.create_transaction();
    state::reset_blocks_count(&transaction)?;
    state::reset_fees(&transaction)?;
    transaction.with_pools(|iterator| {
        for (_, pool) in iterator {
            amaru_ledger::store::columns::pools::Row::tick(pool, epoch + 1);
        }
    })?;
    transaction.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken))?;
    transaction.commit()?;

    let tx: KeepRaw<'_, MintedTx<'_>> = minicbor::decode(record.cbor.raw_bytes())?;

    let utxo = db.iter_utxos()?.collect();
    let mut validation_context = DefaultValidationContext::new(utxo);

    let protocol_parameters = db.get_protocol_parameters_for(&epoch)?;

    let pointer = TransactionPointer {
        slot: point.slot_or_default(),
        transaction_index: 0,
    };

    let tx_body: KeepRaw<'_, MintedTransactionBody<'_>> = tx.transaction_body.clone();
    let tx_witness_set: MintedWitnessSet<'_> = tx.transaction_witness_set.deref().clone();
    let tx_auxiliary_data =
        Into::<Option<KeepRaw<'_, AuxiliaryData>>>::into(tx.auxiliary_data.clone())
            .map(|aux_data| Hasher::<256>::hash(aux_data.raw_cbor()));

    // Run the transaction against the imported ledger state
    let result = transaction::execute(
        &mut validation_context,
        &protocol_parameters,
        pointer,
        true,
        tx_body,
        &tx_witness_set,
        tx_auxiliary_data,
    );

    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            if !record.success {
                Ok(())
            } else {
                Err(format!("Expected success, got failure: {}", e).into())
            }
        }
    }
}
