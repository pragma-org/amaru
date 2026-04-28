// Copyright 2026 PRAGMA
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

use amaru_kernel::{Hash, Transaction, TransactionBody, TransactionInput, WitnessSet, size::TRANSACTION_BODY};
use amaru_ouroboros::{MempoolInsertError, MempoolMsg, ResourceMempool, TxInsertResult, TxOrigin};
use pure_stage::{
    DeserializerGuards, Effect, ExternalEffect, StageGraph, UnknownExternalEffect,
    serde::SendDataValue,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::Runtime;
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::*;
use crate::{
    effects::{ResourceTxValidation, ValidateTxEffect},
    stages::test_utils::{BufferWriter, Logs, TraceMatch},
};

pub struct TestPrep {
    pub msg: MempoolMsg,
    pub rt: Runtime,
    pub mempool: ResourceMempool<Transaction>,
    pub validator: ResourceTxValidation,
}

#[derive(serde::Serialize)]
struct InsertEffectPayload {
    tx: Transaction,
    tx_origin: TxOrigin,
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<MempoolStageState>().boxed(),
        pure_stage::register_data_deserializer::<MempoolMsg>().boxed(),
        pure_stage::register_data_deserializer::<Result<Vec<TxInsertResult>, MempoolInsertError>>().boxed(),
        pure_stage::register_effect_deserializer::<ValidateTxEffect>().boxed(),
    ]
}

pub fn setup(prep: &TestPrep) -> (SimulationRunning, DeserializerGuards, Logs) {
    let writer = BufferWriter::new();
    let mut logs = writer.clone();

    let sub = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();
    logs.set_guard(sub);

    let guards = register_guards();

    let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1_000_000));
    network.resources().put::<ResourceMempool<Transaction>>(prep.mempool.clone());
    network.resources().put::<ResourceTxValidation>(prep.validator.clone());

    let mempool = network.stage("mempool", stage);
    let mempool = network.wire_up(mempool, MempoolStageState::default());
    network.preload(&mempool, [prep.msg.clone()]).unwrap();

    let mut running = network.run();
    running.run_until_blocked_incl_effects(prep.rt.handle());

    (running, guards, logs.logs())
}

pub fn tm_validate_tx(at_stage: &str, tx: &Transaction) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ValidateTxEffect::new(tx)))).into()
}

pub fn tm_insert(at_stage: &str, tx: &Transaction, tx_origin: TxOrigin) -> TraceMatch<'static> {
    let payload = InsertEffectPayload { tx: tx.clone(), tx_origin };
    let value = SendDataValue::from_json("payload", &payload).cast::<SendDataValue>().unwrap().value;
    let effect: Box<dyn ExternalEffect> = Box::new(UnknownExternalEffect::new(SendDataValue {
        typetag: "amaru_protocols::mempool_effects::Insert".to_string(),
        value,
    }));
    TraceEntry::suspend(Effect::external(at_stage, effect)).into()
}

pub fn tm_send(
    from: impl AsRef<str>,
    to: impl AsRef<str>,
    msg: Result<Vec<TxInsertResult>, MempoolInsertError>,
) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::send(from, to, Box::new(msg))).into()
}

pub fn create_transaction(input_index: usize) -> Transaction {
    let tx_input = TransactionInput { transaction_id: Hash::new([1; TRANSACTION_BODY]), index: input_index as u64 };
    let body = TransactionBody::new([tx_input], [], 0);
    Transaction { body, witnesses: WitnessSet::default(), is_expected_valid: true, auxiliary_data: None }
}
