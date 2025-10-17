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
    AnyCbor, AuxiliaryData, Bytes, Epoch, EraHistory, Hasher, KeepRaw, MintedTransactionBody, MintedTx,
    MintedWitnessSet, Point, PoolId, TransactionPointer, cbor, network::NetworkName, Network,
    protocol_parameters::ProtocolParameters,
};
use amaru_ledger::{
    self,
    bootstrap::import_initial_snapshot,
    context::DefaultValidationContext,
    rules::transaction,
    state::{self, volatile_db::StoreUpdate},
    store::{EpochTransitionProgress, ReadStore, Store, TransactionalContext},
};
use amaru_stores::rocksdb::RocksDB;
use std::{collections::BTreeSet, env, fs, iter, ops::Deref, path::PathBuf};

use test_case::test_case;

use once_cell::sync::Lazy;
use std::sync::Mutex;

struct TestContext {
    ledger_dir: PathBuf,
    pparams_dir: PathBuf,
}

fn get_test_context() -> Result<TestContext, Box<dyn std::error::Error>> {
    let working_dir = env::current_dir()?;
    Ok(TestContext {
        ledger_dir: working_dir.join(PathBuf::from("../../ledger.db")),
        pparams_dir: working_dir.join(PathBuf::from("../../cardano-blueprint/src/ledger/conformance-test-vectors/eras/conway/impl/dump/pparams-by-hash/")),
    })
}

static TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(Mutex::default);

include!("generated_ledger_conformance_test_cases.incl");

fn evaluate_vector(snapshot: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _shared = TEST_MUTEX.lock()?;
    let network = NetworkName::Testnet(1);
    let era_history = network.into();
    match get_test_context() {
        Ok(tc) => {
            let vector_file = fs::read(snapshot)?;
            let record: TestVector = minicbor::decode(&vector_file)?;
            let () = import_vector(record, &tc.ledger_dir, era_history, &tc.pparams_dir)?;
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
struct TestVector {
    #[n(0)]
    config: AnyCbor,
    #[n(1)]
    initial_state: AnyCbor,
    #[n(2)]
    final_state: AnyCbor,
    #[n(3)]
    events: Vec<TestVectorEvent>,
    #[n(4)]
    title: String,
}

enum TestVectorEvent {
    Transaction(Bytes, bool, u64),
    PassTick(u64),
    PassEpoch(u64),
}

impl<'b, C> minicbor::decode::Decode<'b, C> for TestVectorEvent {
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.array()?;
        let variant = d.u16()?;

        match variant {
            0 => Ok(TestVectorEvent::Transaction(d.decode_with(ctx)?, d.decode_with(ctx)?, d.decode_with(ctx)?)),
            1 => Ok(TestVectorEvent::PassTick(d.decode_with(ctx)?)),
            2 => Ok(TestVectorEvent::PassEpoch(d.decode_with(ctx)?)),
            _ => Err(minicbor::decode::Error::message(
                "invalid variant id for TestVectorEvent",
            )),
        }
    }
}

fn import_vector(
    record: TestVector,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
    pparams_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let epoch = peek_epoch(&record.initial_state)?;
    let point = Point::Specific(
        epoch * 86400,
        hex::decode("0000000000000000000000000000000000000000000000000000000000000000")?
    );

    fs::create_dir_all(ledger_dir)?;
    let db = RocksDB::empty(ledger_dir)?;

    let network = NetworkName::Testnet(1);
    let epoch = import_initial_snapshot(
        &db,
        &record.initial_state,
        &point,
        network,
        amaru_progress_bar::no_progress_bar,
        Some(pparams_dir),
        false,
    )?;

    let protocol_parameters = db.protocol_parameters()?;

    let mut governance_activity = db.governance_activity()?;

    for (ix, event) in record.events.into_iter().enumerate() {
        let (tx_bytes, success, slot): (Bytes, bool, u64) = match event {
            TestVectorEvent::Transaction(tx, success, slot) => (tx, success, slot),
            _ => continue,
        };
        let tx: MintedTx<'_> = minicbor::decode(&*tx_bytes)?;

        let utxo = db.iter_utxos()?.collect();
        let mut validation_context = DefaultValidationContext::new(utxo);

        let tx_body: KeepRaw<'_, MintedTransactionBody<'_>> = tx.transaction_body.clone();
        let tx_witness_set: MintedWitnessSet<'_> = tx.transaction_witness_set.deref().clone();
        let tx_auxiliary_data =
            Into::<Option<KeepRaw<'_, AuxiliaryData>>>::into(tx.auxiliary_data.clone())
                .map(|aux_data| Hasher::<256>::hash(aux_data.raw_cbor()));

        let pointer = TransactionPointer {
            slot: point.slot_or_default(),
            transaction_index: ix,
        };

        // Run the transaction against the imported ledger state
        let result = transaction::execute(
            &mut validation_context,
            &Network::Testnet,
            &protocol_parameters,
            era_history,
            &governance_activity,
            pointer,
            true,
            tx_body,
            &tx_witness_set,
            tx_auxiliary_data,
        );

        let vs = state::VolatileState::from(validation_context);

        // Can we re-use this point? We should be able to treat all txes as if they're in the same block
        let p: Point = point.clone();

        let issuer: PoolId = [7; 28].into();
        let anchored_vs = vs.anchor(&p, issuer);
        let StoreUpdate {
            point: stable_point,
            issuer: stable_issuer,
            fees,
            add,
            remove,
            withdrawals,
        } = anchored_vs.into_store_update(epoch, &protocol_parameters);

        let mut governance_activity = db.governance_activity()?;
        let transaction = db.create_transaction();
        transaction.save(
            &era_history,
            &protocol_parameters,
            &mut governance_activity,
            &stable_point,
            Some(&stable_issuer),
            add,
            remove,
            withdrawals,
        );
        transaction.commit()?;

        match result {
            Ok(()) => {
                if success {
                    // Ok(())
                } else {
                    return Err(format!("Expected failure, got success").into());
                }
            }
            Err(e) => {
                if !success {
                    // Ok(())
                } else {
                    return Err(format!("Expected success, got failure: {}", e).into());
                }
            }
        }
    }
    Ok(())
}
