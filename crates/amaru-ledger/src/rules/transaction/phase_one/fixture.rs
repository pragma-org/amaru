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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    CertificatePointer, DRep, DRepRegistration, Epoch, EraHistoryProxy, Lovelace, MemoizedTransactionOutput,
    NetworkName, PoolId, ProtocolParameters, StakeCredential, TransactionInput, TransactionPointer, cbor, json,
    utils::serde::{RefOrInline, deserialize_utxo, hex_to_bytes},
};
use serde::Deserialize;

use crate::{
    context::{AccountState, DelegateError},
    epoch_transition::GovernanceActivity,
    rules::{
        WithPosition,
        transaction::phase_one::{
            InvalidCertificates, InvalidFees, InvalidInputs, InvalidTransactionMetadata, InvalidVKeyWitness,
            InvalidValidityInterval, InvalidWithdrawals, PhaseOneError,
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
    pub(super) pools: BTreeSet<PoolId>,
    #[serde(deserialize_with = "deserialize_accounts")]
    pub(super) accounts: BTreeMap<StakeCredential, AccountState>,
    #[serde(deserialize_with = "deserialize_dreps")]
    pub(super) dreps: BTreeMap<StakeCredential, DRepRegistration>,
    pub(super) governance_activity: GovernanceActivity,
}

fn deserialize_cbor_hex<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: for<'b> cbor::Decode<'b, ()>,
{
    let hex = String::deserialize(deserializer)?;
    let bytes = hex::decode(hex).map_err(serde::de::Error::custom)?;
    cbor::decode(&bytes).map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolDelegationProxy {
    id: PoolId,
    delegated_at: CertificatePointer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoteDelegationProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    id: DRep,
    delegated_at: CertificatePointer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    credential: StakeCredential,
    deposit: Lovelace,
    #[serde(default)]
    rewards: Lovelace,
    #[serde(default)]
    pool: Option<PoolDelegationProxy>,
    #[serde(default)]
    drep: Option<VoteDelegationProxy>,
}

fn deserialize_accounts<'de, D>(deserializer: D) -> Result<BTreeMap<StakeCredential, AccountState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries = Vec::<AccountProxy>::deserialize(deserializer)?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            let state = AccountState {
                deposit: entry.deposit,
                pool: entry.pool.map(|pool| (pool.id, pool.delegated_at)),
                drep: entry.drep.map(|drep| (drep.id, drep.delegated_at)),
                rewards: entry.rewards,
            };
            (entry.credential, state)
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DRepProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    credential: StakeCredential,
    deposit: Lovelace,
    registered_at: CertificatePointer,
    valid_until: Epoch,
}

fn deserialize_dreps<'de, D>(deserializer: D) -> Result<BTreeMap<StakeCredential, DRepRegistration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries = Vec::<DRepProxy>::deserialize(deserializer)?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            let registration = DRepRegistration {
                deposit: entry.deposit,
                registered_at: entry.registered_at,
                valid_until: entry.valid_until,
            };
            (entry.credential, registration)
        })
        .collect())
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
    DelegateeStakePoolNotRegistered,
    StakeCredentialInvalidPoolDelegation,
    StakeCredentialInvalidVoteDelegation,
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
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialInvalidPoolDelegation(ref e)) => match e {
                DelegateError::UnknownSource(_) => Predicate::StakeCredentialInvalidPoolDelegation,
                DelegateError::UnknownTarget(_) => Predicate::DelegateeStakePoolNotRegistered,
            },
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialInvalidVoteDelegation(_)) => {
                Predicate::StakeCredentialInvalidVoteDelegation
            }
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
