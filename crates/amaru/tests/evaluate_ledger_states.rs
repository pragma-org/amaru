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
    MintedWitnessSet, Point, PoolId, TransactionInput, TransactionOutput, TransactionPointer, cbor, network::NetworkName, Network, Value,
    protocol_parameters::ProtocolParameters,
};
use amaru_ledger::{
    self,
    context::DefaultValidationContext,
    rules::transaction,
    state::{self, volatile_db::StoreUpdate},
    store::{EpochTransitionProgress, GovernanceActivity, ReadStore, Store, TransactionalContext},
};
use amaru_stores::rocksdb::RocksDB;
use std::{collections::{BTreeMap, BTreeSet}, env, fs, iter, ops::Deref, path::PathBuf};

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

// Traverse into the NewEpochState and find the utxos. In practice the initial ledger state for
// each vector is very simple so we don't need to worry about every field. We really just care
// about the utxos.
fn decode_ledger_state<'b>(d: &mut minicbor::Decoder<'b>) -> Result<(DefaultValidationContext, &'b minicbor::bytes::ByteSlice, GovernanceActivity), minicbor::decode::Error> {
    let _begin_nes = d.array()?;
    let _epoch_no = d.u64()?;
    let _blocks_made  = d.skip()?;
    let _blocks_made = d.skip()?;
    let _begin_epoch_state = d.array()?;
    let _begin_account_state = d.skip()?;
    let _begin_ledger_state = d.array()?;
    let _cert_state = d.array()?;
    let _voting_state = d.array()?;
    let _dreps = d.skip()?;
    let _committee_state = d.skip()?;
    let number_of_dormant_epochs: Epoch = d.decode()?;
    let _p_state = d.skip()?;
    let _d_state = d.skip()?;
    let _utxo_state = d.array()?;

    let mut utxos_map = BTreeMap::new();
    let utxos_map_count = d.map()?;
    match utxos_map_count {
        Some(n) => {
            for _ in 0..n {
                let tx_in = d.decode()?;
                let tx_out = d.decode()?;
                utxos_map.insert(tx_in, tx_out);
            }
        }
        None => {
            loop {
                let ty = d.datatype()?;
                if ty == minicbor::data::Type::Break {
                    break;
                }
                let tx_in = d.decode()?;
                let tx_out = d.decode()?;
                utxos_map.insert(tx_in, tx_out);
            }
        }
    }
    let _deposited = d.skip()?;
    let _fees = d.skip()?;

    let _gov_state = d.array()?;
    let _proposals = d.skip()?;
    let _committee = d.skip()?;
    let _constitution = d.skip()?;
    let current_pparams_hash = d.decode()?;
    let _previous_pparams_hash = d.skip()?;
    let _future_pparams = d.skip()?;
    let _drep_pulsing_state = d.skip()?;

    let _stake_distr = d.skip()?;
    let _donation = d.skip()?;
    let _snapshots = d.skip()?;
    let _non_myopic = d.skip()?;
    let _pulsing_rew = d.skip()?;
    let _pool_distr = d.skip()?;
    let _stashed = d.skip()?;

    Ok((
        DefaultValidationContext::new(utxos_map),
        current_pparams_hash,
        GovernanceActivity {
            consecutive_dormant_epochs: u64::from(number_of_dormant_epochs) as u32,
        }
    ))
}

fn decode_segregated_parameters(
    dir: &PathBuf,
    hash: &minicbor::bytes::ByteSlice,
) -> Result<ProtocolParameters, Box<dyn std::error::Error>> {
    let pparams_file_path = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| {
            path.file_name()
                .map(|filename| filename.to_str() == Some(&hex::encode(hash.as_ref())))
                .unwrap_or(false)
        })
        .ok_or("Missing pparams file")?;

    let pparams_file = fs::read(pparams_file_path)?;

    let pparams = cbor::Decoder::new(&pparams_file).decode()?;

    Ok(pparams)
}

fn import_vector(
    record: TestVector,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
    pparams_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let epoch = peek_epoch(&record.initial_state)?;

    let mut decoder = minicbor::Decoder::new(&record.initial_state);
    let (mut validation_context, pparams_hash, governance_activity) = decode_ledger_state(&mut decoder)?;

    let protocol_parameters = decode_segregated_parameters(pparams_dir, &pparams_hash)?;

    for (ix, event) in record.events.into_iter().enumerate() {
        let (tx_bytes, success, slot): (Bytes, bool, u64) = match event {
            TestVectorEvent::Transaction(tx, success, slot) => (tx, success, slot),
            _ => continue,
        };

        let point = Point::Specific(
            slot,
            hex::decode("0000000000000000000000000000000000000000000000000000000000000000")?
        );

        let tx: MintedTx<'_> = minicbor::decode(&*tx_bytes)?;

        let tx_body: KeepRaw<'_, MintedTransactionBody<'_>> = tx.transaction_body.clone();
        let tx_witness_set: MintedWitnessSet<'_> = tx.transaction_witness_set.deref().clone();
        let tx_auxiliary_data =
            Into::<Option<KeepRaw<'_, AuxiliaryData>>>::into(tx.auxiliary_data.clone())
                .map(|aux_data| Hasher::<256>::hash(aux_data.raw_cbor()));

        let pointer = TransactionPointer {
            slot: point.slot_or_default(),
            // Using the loop index here is conterintuitive but ensures that tx pointers will be distinct even if
            // the slots are the same. ultimately the pointers are made up since we do not have real blocks
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
