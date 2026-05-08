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

use std::{collections::BTreeMap, str::FromStr, time::Duration};

use amaru_kernel::{
    Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, MemoizedTransactionOutput, NetworkName,
    ProtocolParameters, Slot, TransactionInput, TransactionPointer, cbor, json, utils::serde::hex_to_bytes,
};
use serde::Deserialize;

use crate::{
    rules::transaction::phase_one::{InvalidVKeyWitness, PhaseOneError},
    store::GovernanceActivity,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Fixture {
    #[serde(deserialize_with = "deserialize_network_name")]
    pub(super) network: NetworkName,
    pub(super) era_history: EraHistoryFixture,
    pub(super) protocol_parameters: ProtocolParameters,
    pub(super) initial_state: InitialState,
    pub(super) ledger_env: LedgerEnv,
    #[serde(deserialize_with = "hex_to_bytes")]
    pub(super) transaction: Vec<u8>,
    pub(super) expected: Expected,
}

#[derive(Debug)]
pub(super) enum Expected {
    Pass,
    Fail(Predicate),
}

impl<'de> Deserialize<'de> for Expected {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = json::Value::deserialize(d)?;
        match value {
            json::Value::String(s) if s == "Pass" => Ok(Expected::Pass),
            json::Value::Object(_) => json::from_value(value).map(Expected::Fail).map_err(serde::de::Error::custom),
            json::Value::String(s) => Err(serde::de::Error::custom(format!("expected \"Pass\", got {s:?}"))),
            json::Value::Null | json::Value::Bool(_) | json::Value::Number(_) | json::Value::Array(_) => {
                Err(serde::de::Error::custom("expected \"Pass\" or { predicate: ..., ... }"))
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "predicate")]
pub(super) enum Predicate {
    InvalidWitnessesUTXOW,
}

impl From<PhaseOneError> for Predicate {
    fn from(err: PhaseOneError) -> Self {
        match err {
            PhaseOneError::VKeyWitness(InvalidVKeyWitness::InvalidSignatures { .. }) => {
                Predicate::InvalidWitnessesUTXOW
            }
            PhaseOneError::VKeyWitness(_)
            | PhaseOneError::Inputs(_)
            | PhaseOneError::Outputs(_)
            | PhaseOneError::Certificates(_)
            | PhaseOneError::Fees(_)
            | PhaseOneError::Withdrawals(_)
            | PhaseOneError::Scripts(_)
            | PhaseOneError::Collateral(_)
            | PhaseOneError::Proposals(_)
            | PhaseOneError::Metadata(_)
            | PhaseOneError::InvalidNetworkID { .. }
            | PhaseOneError::TooLarge { .. } => unreachable!("no predicate mapping yet for {err}"),
        }
    }
}

fn deserialize_network_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<NetworkName, D::Error> {
    let s = String::deserialize(d)?;
    NetworkName::from_str(&s).map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitialState {
    #[serde(deserialize_with = "deserialize_utxo")]
    pub(super) utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    pub(super) voting_state: VotingState,
}

#[derive(Debug, Deserialize)]
struct UtxoEntryFixture {
    #[serde(deserialize_with = "hex_to_bytes")]
    input: Vec<u8>,
    #[serde(deserialize_with = "hex_to_bytes")]
    output: Vec<u8>,
}

fn deserialize_utxo<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<TransactionInput, MemoizedTransactionOutput>, D::Error> {
    let entries = Vec::<UtxoEntryFixture>::deserialize(d)?;
    entries
        .into_iter()
        .map(|entry| {
            let input: TransactionInput = cbor::decode(&entry.input).map_err(serde::de::Error::custom)?;
            let output: MemoizedTransactionOutput = cbor::decode(&entry.output).map_err(serde::de::Error::custom)?;
            Ok((input, output))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VotingState {
    num_dormant_epochs: u32,
}

impl From<VotingState> for GovernanceActivity {
    fn from(v: VotingState) -> Self {
        GovernanceActivity { consecutive_dormant_epochs: v.num_dormant_epochs }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LedgerEnv {
    slot: Slot,
    tx_ix: u64,
}

impl From<LedgerEnv> for TransactionPointer {
    fn from(env: LedgerEnv) -> Self {
        TransactionPointer { slot: env.slot, transaction_index: env.tx_ix.try_into().expect("tx_ix fits in usize") }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EraHistoryFixture {
    stability_window: Slot,
    eras: Vec<EraSummaryFixture>,
}

impl From<EraHistoryFixture> for EraHistory {
    fn from(f: EraHistoryFixture) -> Self {
        let eras: Vec<EraSummary> = f.eras.into_iter().map(Into::into).collect();
        EraHistory::new(&eras, f.stability_window)
    }
}

#[derive(Debug, Deserialize)]
struct EraSummaryFixture {
    start: EraBoundFixture,
    end: Option<EraBoundFixture>,
    params: EraParamsFixture,
}

impl From<EraSummaryFixture> for EraSummary {
    fn from(f: EraSummaryFixture) -> Self {
        EraSummary { start: f.start.into(), end: f.end.map(Into::into), params: f.params.into() }
    }
}

#[derive(Debug, Deserialize)]
struct EraBoundFixture {
    time: u64,
    slot: u64,
    epoch: u64,
}

impl From<EraBoundFixture> for EraBound {
    fn from(f: EraBoundFixture) -> Self {
        EraBound { time: Duration::from_secs(f.time), slot: Slot::new(f.slot), epoch: Epoch::new(f.epoch) }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EraParamsFixture {
    epoch_size_slots: u64,
    slot_length_ms: u64,
    era_name: EraName,
}

impl From<EraParamsFixture> for EraParams {
    fn from(f: EraParamsFixture) -> Self {
        EraParams {
            epoch_size_slots: f.epoch_size_slots,
            slot_length: Duration::from_millis(f.slot_length_ms),
            era_name: f.era_name,
        }
    }
}
