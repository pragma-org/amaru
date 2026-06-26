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

use std::collections::BTreeMap;

use amaru_kernel::{
    EraHistoryProxy, MemoizedTransactionOutput, NetworkName, ProtocolParameters, TransactionInput, TransactionPointer,
    json,
    utils::serde::{RefOrInline, deserialize_utxo, hex_to_bytes},
};
use serde::Deserialize;

use crate::{
    epoch_transition::GovernanceActivity,
    rules::{
        WithPosition,
        transaction::phase_one::{
            InvalidFees, InvalidInputs, InvalidTransactionMetadata, InvalidVKeyWitness, InvalidValidityInterval,
            InvalidWithdrawals, PhaseOneError,
            outputs::{InvalidOutput, InvalidOutputs},
        },
    },
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Fixture {
    pub(super) network: NetworkName,
    pub(super) era_history: RefOrInline<EraHistoryProxy>,
    pub(super) protocol_parameters: RefOrInline<ProtocolParameters>,
    pub(super) initial_state: InitialState,
    pub(super) point: TransactionPointer,
    #[serde(deserialize_with = "hex_to_bytes")]
    pub(super) transaction: Vec<u8>,
    pub(super) expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitialState {
    #[serde(deserialize_with = "deserialize_utxo")]
    pub(super) utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    pub(super) governance_activity: GovernanceActivity,
}

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
    BabbageNonDisjointRefInputs,
    BabbageOutputTooSmallUTxO,
    BadInputsUTxO,
    ConflictingMetadataHash,
    ConwayTxRefScriptsSizeTooBig,
    FeeTooSmallUTxO,
    InputSetEmptyUTxO,
    InsufficientCollateral,
    InvalidWitnessesUTXOW,
    MaxTxSizeUTxO,
    MissingTxBodyMetadataHash,
    MissingTxMetadata,
    MissingVKeyWitnessesUTXOW,
    OutputTooBigUTxO,
    OutsideForecast,
    OutsideValidityIntervalUTxO,
    ValueNotConservedUTxO,
    WrongNetworkInTxBody,
    WrongNetworkInTxOutput,
    WrongNetworkWithdrawal,
}

impl From<PhaseOneError> for Predicate {
    fn from(err: PhaseOneError) -> Self {
        match err {
            PhaseOneError::VKeyWitness(InvalidVKeyWitness::InvalidSignatures { .. }) => {
                Predicate::InvalidWitnessesUTXOW
            }
            PhaseOneError::VKeyWitness(InvalidVKeyWitness::MissingRequiredKeysOrRoots { .. }) => {
                Predicate::MissingVKeyWitnessesUTXOW
            }
            PhaseOneError::Withdrawals(InvalidWithdrawals::NetworkMismatch { .. }) => Predicate::WrongNetworkWithdrawal,
            PhaseOneError::Metadata(InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(_)) => {
                Predicate::MissingTxBodyMetadataHash
            }
            PhaseOneError::Metadata(InvalidTransactionMetadata::MissingTransactionMetadata(_)) => {
                Predicate::MissingTxMetadata
            }
            PhaseOneError::Metadata(InvalidTransactionMetadata::ConflictingMetadataHash { .. }) => {
                Predicate::ConflictingMetadataHash
            }
            PhaseOneError::Inputs(InvalidInputs::EmptyInputSet) => Predicate::InputSetEmptyUTxO,
            PhaseOneError::Inputs(InvalidInputs::UnknownInput(_)) => Predicate::BadInputsUTxO,
            PhaseOneError::Inputs(InvalidInputs::NonDisjointRefInputs { .. }) => Predicate::BabbageNonDisjointRefInputs,
            PhaseOneError::Inputs(InvalidInputs::RefScriptSizeTooBig { .. }) => Predicate::ConwayTxRefScriptsSizeTooBig,
            PhaseOneError::Fees(InvalidFees::FeeTooSmall { .. }) => Predicate::FeeTooSmallUTxO,
            PhaseOneError::Fees(InvalidFees::UnknownCollateralInput { .. }) => Predicate::BadInputsUTxO,
            PhaseOneError::Fees(InvalidFees::CollateralReturnOverflow { .. }) => Predicate::InsufficientCollateral,
            PhaseOneError::InvalidNetworkID { .. } => Predicate::WrongNetworkInTxBody,
            PhaseOneError::TooLarge { .. } => Predicate::MaxTxSizeUTxO,
            PhaseOneError::ValidityInterval(InvalidValidityInterval::OutsideValidityInterval { .. }) => {
                Predicate::OutsideValidityIntervalUTxO
            }
            PhaseOneError::ValidityInterval(InvalidValidityInterval::OutsideForecast(_)) => Predicate::OutsideForecast,
            PhaseOneError::Outputs(InvalidOutputs { ref invalid_outputs }) => match invalid_outputs.as_slice() {
                [WithPosition { element: InvalidOutput::TooSmall { .. }, .. }] => Predicate::BabbageOutputTooSmallUTxO,
                [WithPosition { element: InvalidOutput::ValueTooLarge { .. }, .. }] => Predicate::OutputTooBigUTxO,
                [WithPosition { element: InvalidOutput::WrongNetwork { .. }, .. }] => Predicate::WrongNetworkInTxOutput,
                _ => unreachable!("no predicate mapping yet for {err}"),
            },
            PhaseOneError::ValueNotPreserved(_) => Predicate::ValueNotConservedUTxO,
            PhaseOneError::Inputs(_)
            | PhaseOneError::Metadata(_)
            | PhaseOneError::VKeyWitness(_)
            | PhaseOneError::Certificates(_)
            | PhaseOneError::Withdrawals(_)
            | PhaseOneError::Scripts(_)
            | PhaseOneError::Collateral(_)
            | PhaseOneError::Proposals(_) => unreachable!("no predicate mapping yet for {err}"),
        }
    }
}
