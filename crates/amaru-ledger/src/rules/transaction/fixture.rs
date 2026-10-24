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
    CertificatePointer, ConstitutionalCommitteeMemberStatus, Credential, DRep, DRepRegistration, Epoch, EraHistory,
    Hash, Lovelace, MemoizedTransactionOutput, NetworkName, PoolId, Pots, ProposalId, ProposalSlim, ProposalsRoots,
    ProtocolParameters, TransactionInput, TransactionPointer, cbor, json,
    size::SCRIPT,
    utils::serde::{RefOrInline, deserialize_utxo, hex_to_bytes},
};
use serde::Deserialize;

use crate::{
    context::{AccountState, CCMember, DelegateError, ProposalStateSlim},
    epoch_transition::GovernanceActivity,
    rules::{
        WithPosition,
        block::TransactionInvalid,
        transaction::{
            phase_one::{
                InvalidCertificates, InvalidCollateral, InvalidFees, InvalidInputs, InvalidScripts,
                InvalidTransactionMetadata, InvalidValidityInterval, InvalidVerificationKeyWitness,
                InvalidVotingProcedures, InvalidWithdrawals, PhaseOneError,
                outputs::{InvalidOutput, InvalidOutputs},
                proposals::InvalidProposals,
            },
            phase_two::{PreparationError, TagMismatch},
        },
    },
};

#[derive(Deserialize)]
pub(super) struct Fixture {
    pub(super) network: NetworkName,
    pub(super) era_history: RefOrInline<EraHistory>,
    pub(super) protocol_parameters: RefOrInline<ProtocolParameters>,
    pub(super) initial_state: InitialState,
    pub(super) point: TransactionPointer,
    #[serde(deserialize_with = "hex_to_bytes")]
    pub(super) transaction: Vec<u8>,
    pub(super) expected: Expected,
}

#[derive(Deserialize)]
pub(super) struct InitialState {
    #[serde(deserialize_with = "deserialize_utxo", default)]
    pub(super) utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    #[serde(default)]
    pub(super) pools: BTreeSet<PoolId>,
    #[serde(deserialize_with = "deserialize_accounts", default)]
    pub(super) accounts: BTreeMap<Credential, AccountState>,
    #[serde(deserialize_with = "deserialize_dreps", default)]
    pub(super) dreps: BTreeMap<Credential, DRepRegistration>,
    #[serde(deserialize_with = "deserialize_committee", default)]
    pub(super) committee: BTreeMap<Credential, CCMember>,
    #[serde(deserialize_with = "deserialize_proposals", default)]
    pub(super) proposals: BTreeMap<ProposalId, ProposalStateSlim>,
    #[serde(default)]
    pub(super) proposals_roots: ProposalsRoots,
    #[serde(default)]
    pub(super) governance_activity: GovernanceActivity,
    #[serde(default)]
    pub(super) voting_state: Option<VotingState>,
    #[serde(default)]
    pub(super) pots: Pots,
    /// Guardrails script of the enacted constitution, if it has one. A proposal carrying a policy
    /// must name exactly this script; absent or `null` requires proposals to carry no policy at all.
    #[serde(default)]
    pub(super) guardrail_script: Option<Hash<SCRIPT>>,
}

/// The block-start voting state used by governance validation. The legacy fixture shape keeps
/// these fields at the top level; the grouped form lets imported ledger snapshots carry them as a
/// single state without changing existing fixtures.
#[derive(Deserialize)]
pub(super) struct VotingState {
    #[serde(deserialize_with = "deserialize_dreps", default)]
    pub(super) dreps: BTreeMap<Credential, DRepRegistration>,
    #[serde(deserialize_with = "deserialize_committee", default)]
    pub(super) committee: BTreeMap<Credential, CCMember>,
    #[serde(default)]
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
struct PoolDelegationProxy {
    id: PoolId,
    delegated_at: CertificatePointer,
}

#[derive(Deserialize)]
struct VoteDelegationProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    id: DRep,
    delegated_at: CertificatePointer,
}

#[derive(Deserialize)]
struct AccountProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    credential: Credential,
    deposit: Lovelace,
    #[serde(default)]
    rewards: Lovelace,
    #[serde(default)]
    pool: Option<PoolDelegationProxy>,
    #[serde(default)]
    drep: Option<VoteDelegationProxy>,
}

fn deserialize_accounts<'de, D>(deserializer: D) -> Result<BTreeMap<Credential, AccountState>, D::Error>
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
struct DRepProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    credential: Credential,
    deposit: Lovelace,
    registered_at: CertificatePointer,
    valid_until: Epoch,
}

fn deserialize_dreps<'de, D>(deserializer: D) -> Result<BTreeMap<Credential, DRepRegistration>, D::Error>
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

fn deserialize_proposals<'de, D>(deserializer: D) -> Result<BTreeMap<ProposalId, ProposalStateSlim>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Vec::<(ProposalId, ProposalSlim, Epoch)>::deserialize(deserializer)?
        .into_iter()
        .map(|(id, action, valid_until)| (id, ProposalStateSlim { action, valid_until }))
        .collect())
}

/// A row of the constitutional committee, keyed by cold credential. `hotCredential` is absent for a
/// member that has never authorized one or has resigned; `validUntil` is absent for a member that is
/// not (or no longer) elected, which is a state a member can still authorize a hot key from.
#[derive(Deserialize)]
struct CommitteeMemberProxy {
    #[serde(deserialize_with = "deserialize_cbor_hex")]
    cold_credential: Credential,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    valid_until: Option<Epoch>,
}

fn deserialize_committee<'de, D>(deserializer: D) -> Result<BTreeMap<Credential, CCMember>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<CommitteeMemberProxy>::deserialize(deserializer)?
        .into_iter()
        .map(|entry| {
            let status = if let Some(st) = entry.status {
                Some(if st == "resigned" {
                    ConstitutionalCommitteeMemberStatus::Resigned
                } else {
                    let hot_credential = cbor::from_cbor(&hex::decode(&st).map_err(serde::de::Error::custom)?)
                        .ok_or_else(|| serde::de::Error::custom("unable to decode hot credential"))?;
                    ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(hot_credential)
                })
            } else {
                None
            };

            let member = CCMember { status, valid_until: entry.valid_until };
            Ok((entry.cold_credential, member))
        })
        .collect::<Result<_, _>>()
}

#[derive(Debug)]
pub(super) enum Expected {
    Pass,
    DecodingFailure,
    Fail(Predicate),
}

impl<'de> Deserialize<'de> for Expected {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = json::Value::deserialize(d)?;
        match value {
            json::Value::String(s) if s == "Pass" => Ok(Expected::Pass),
            json::Value::Object(ref map) if map.contains_key("decoding_failure") => match map.get("decoding_failure") {
                Some(json::Value::Bool(true)) if map.contains_key("predicate") => Err(serde::de::Error::custom(
                    "expected.decoding_failure cannot be combined with a predicate: no validation runs, so the predicate would never be checked",
                )),
                Some(json::Value::Bool(true)) => Ok(Expected::DecodingFailure),
                Some(other) => Err(serde::de::Error::custom(format!(
                    "expected.decoding_failure must be `true`, got {other}; omit the field to expect validation to run"
                ))),
                None => unreachable!("guarded by contains_key"),
            },
            json::Value::Object(_) => json::from_value(value).map(Expected::Fail).map_err(serde::de::Error::custom),
            json::Value::String(s) => Err(serde::de::Error::custom(format!("expected \"Pass\", got {s:?}"))),
            json::Value::Null | json::Value::Bool(_) | json::Value::Number(_) | json::Value::Array(_) => Err(
                serde::de::Error::custom("expected \"Pass\", { decoding_failure: true } or { predicate: ..., ... }"),
            ),
        }
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub(super) enum TagMismatchDescription {
    /// The transaction claims to be invalid, yet every script it runs passes. A transaction
    /// carrying no Plutus script at all lands here too: an empty script set trivially passes.
    PassedUnexpectedly,
    /// The transaction claims to be valid, yet at least one of its scripts fails.
    FailedUnexpectedly,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(tag = "predicate")]
pub(super) enum Predicate {
    BabbageNonDisjointRefInputs,
    BabbageOutputTooSmallUTxO,
    BadInputsUTxO,
    CommitteeHasPreviouslyResigned,
    CommitteeIsUnknown,
    ConflictingCommitteeUpdate,
    ConflictingMetadataHash,
    ConwayTreasuryValueMismatch,
    ConwayTxRefScriptsSizeTooBig,
    ConwayWdrlNotDelegatedToDRep,
    CurrentTreasuryFieldNotSupported,
    DRepAlreadyRegistered,
    DRepNotRegistered,
    DelegateeDRepNotRegistered,
    DelegateeStakePoolNotRegistered,
    DisallowedVoters,
    ExpirationEpochTooSmall,
    ExtraRedeemers,
    ExtraneousScriptWitnessesUTXOW,
    FeeTooSmallUTxO,
    GovActionsDoNotExist,
    IncorrectDepositDELEG,
    IncorrectDepositDRep,
    IncorrectTotalCollateralField,
    InlineDatumsNotSupported,
    InputSetEmptyUTxO,
    InsufficientCollateral,
    InvalidGuardrailsScriptHash,
    InvalidMetadata,
    InvalidPrevGovActionId,
    InvalidWitnessesUTXOW,
    MalformedProposal,
    MalformedReferenceScripts,
    MalformedScriptWitnesses,
    MaxTxSizeUTxO,
    MissingCostModel,
    MissingRedeemers,
    MissingRequiredDatums,
    MissingScriptWitnessesUTXOW,
    MissingTxBodyMetadataHash,
    MissingTxMetadata,
    MissingVerificationKeyWitnessesUTXOW,
    NoCollateralInputs,
    NotAllowedSupplementalDatums,
    OutputTooBigUTxO,
    OutsideForecast,
    OutsideValidityIntervalUTxO,
    PPViewHashesDontMatch,
    PoolMetadataHashTooBig,
    ProposalCantFollow,
    ProposalDepositIncorrect,
    ProposalProcedureNetworkIdMismatch,
    ProposalProceduresFieldNotSupported,
    ProposalReturnAccountDoesNotExist,
    ScriptWitnessNotValidatingUTXOW,
    ScriptsNotPaidUTxO,
    StakeCredentialInvalidPoolDelegation,
    StakeCredentialInvalidVoteDelegation,
    StakeKeyHasNonZeroAccountBalance,
    StakeKeyNotRegistered,
    StakeKeyRegistered,
    StakePoolCostTooLowPOOL,
    StakePoolNotRegisteredOnKeyPOOL,
    StakePoolRetirementWrongEpochPOOL,
    TooManyCollateralInputs,
    TooManyExUnits,
    TreasuryDonationFieldNotSupported,
    TreasuryWithdrawalReturnAccountsDoNotExist,
    TreasuryWithdrawalsAllZeros,
    UnspendableUTxONoDatumHash,
    ValidationTagMismatch { description: TagMismatchDescription },
    ValueNotConservedUTxO,
    VotersDoNotExist,
    VotingOnExpiredGovAction,
    VotingProceduresFieldNotSupported,
    WithdrawalsNotInRewardsCERTS,
    WrongNetworkInTxBody,
    WrongNetworkInTxOutput,
    WrongNetworkPOOL,
    WrongNetworkWithdrawal,
}

impl From<TransactionInvalid> for Predicate {
    fn from(err: TransactionInvalid) -> Self {
        match err {
            TransactionInvalid::PhaseOne(err) => Predicate::from(err),
            TransactionInvalid::PhaseTwo(TagMismatch::FailedUnexpectedly(_)) => {
                Predicate::ValidationTagMismatch { description: TagMismatchDescription::FailedUnexpectedly }
            }
            TransactionInvalid::PhaseTwo(TagMismatch::PassedUnexpectedly) => {
                Predicate::ValidationTagMismatch { description: TagMismatchDescription::PassedUnexpectedly }
            }
        }
    }
}

impl From<PhaseOneError> for Predicate {
    fn from(err: PhaseOneError) -> Self {
        match err {
            PhaseOneError::VerificationKeyWitness(InvalidVerificationKeyWitness::InvalidSignatures { .. }) => {
                Predicate::InvalidWitnessesUTXOW
            }
            PhaseOneError::VerificationKeyWitness(InvalidVerificationKeyWitness::MissingRequiredKeysOrRoots {
                ..
            }) => Predicate::MissingVerificationKeyWitnessesUTXOW,
            PhaseOneError::Withdrawals(InvalidWithdrawals::NetworkMismatch { .. }) => Predicate::WrongNetworkWithdrawal,
            PhaseOneError::Withdrawals(InvalidWithdrawals::MissingAccountDRepDelegation(_)) => {
                Predicate::ConwayWdrlNotDelegatedToDRep
            }
            PhaseOneError::Withdrawals(InvalidWithdrawals::AccountNotRegistered(_))
            | PhaseOneError::Withdrawals(InvalidWithdrawals::IncompleteWithdrawal { .. }) => {
                Predicate::WithdrawalsNotInRewardsCERTS
            }
            PhaseOneError::Metadata(InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(_)) => {
                Predicate::MissingTxBodyMetadataHash
            }
            PhaseOneError::Metadata(InvalidTransactionMetadata::MissingTransactionMetadata(_)) => {
                Predicate::MissingTxMetadata
            }
            PhaseOneError::Metadata(InvalidTransactionMetadata::ConflictingMetadataHash { .. }) => {
                Predicate::ConflictingMetadataHash
            }
            PhaseOneError::Metadata(InvalidTransactionMetadata::InvalidScriptBytes(_)) => Predicate::InvalidMetadata,
            PhaseOneError::Inputs(InvalidInputs::EmptyInputSet) => Predicate::InputSetEmptyUTxO,
            PhaseOneError::Inputs(InvalidInputs::UnknownInput(_)) => Predicate::BadInputsUTxO,
            PhaseOneError::Inputs(InvalidInputs::NonDisjointRefInputs { .. }) => Predicate::BabbageNonDisjointRefInputs,
            PhaseOneError::Inputs(InvalidInputs::RefScriptSizeTooBig { .. }) => Predicate::ConwayTxRefScriptsSizeTooBig,
            PhaseOneError::Fees(InvalidFees::FeeTooSmall { .. }) => Predicate::FeeTooSmallUTxO,
            PhaseOneError::InvalidNetwork { .. } => Predicate::WrongNetworkInTxBody,
            PhaseOneError::TooLarge { .. } => Predicate::MaxTxSizeUTxO,
            PhaseOneError::TreasuryValueMismatch { .. } => Predicate::ConwayTreasuryValueMismatch,
            PhaseOneError::ValidityInterval(InvalidValidityInterval::OutsideValidityInterval { .. }) => {
                Predicate::OutsideValidityIntervalUTxO
            }
            PhaseOneError::ValidityInterval(InvalidValidityInterval::OutsideForecast(_)) => Predicate::OutsideForecast,
            PhaseOneError::Certificates(InvalidCertificates::IncorrectStakeDeposit { .. }) => {
                Predicate::IncorrectDepositDELEG
            }
            PhaseOneError::Certificates(InvalidCertificates::IncorrectDRepDeposit { .. }) => {
                Predicate::IncorrectDepositDRep
            }
            PhaseOneError::Outputs(InvalidOutputs { ref invalid_outputs }) => match invalid_outputs.as_slice() {
                [WithPosition { element: InvalidOutput::TooSmall { .. }, .. }] => Predicate::BabbageOutputTooSmallUTxO,
                [WithPosition { element: InvalidOutput::ValueTooLarge { .. }, .. }] => Predicate::OutputTooBigUTxO,
                [WithPosition { element: InvalidOutput::WrongNetwork { .. }, .. }] => Predicate::WrongNetworkInTxOutput,
                [WithPosition { element: InvalidOutput::MalformedReferenceScript(_), .. }] => {
                    Predicate::MalformedReferenceScripts
                }
                // The Haskell node returns `OutputTooBigUTxO` instead of a `OutputBootAddrAttrsTooBig`
                [WithPosition { element: InvalidOutput::BootAddrAttrsTooBig { .. }, .. }] => {
                    Predicate::OutputTooBigUTxO
                }
                _ => unreachable!("no predicate mapping yet for {err}"),
            },
            PhaseOneError::Proposals(InvalidProposals::TreasuryWithdrawalReturnAccountsDoNotExist(_)) => {
                Predicate::TreasuryWithdrawalReturnAccountsDoNotExist
            }
            PhaseOneError::Proposals(InvalidProposals::IncorrectDeposit { .. }) => Predicate::ProposalDepositIncorrect,
            PhaseOneError::Proposals(InvalidProposals::ConflictingCommitteeUpdate) => {
                Predicate::ConflictingCommitteeUpdate
            }
            PhaseOneError::Proposals(InvalidProposals::TreasuryWithdrawalsAllZeros) => {
                Predicate::TreasuryWithdrawalsAllZeros
            }
            PhaseOneError::Proposals(InvalidProposals::ProposalReturnAccountDoesNotExist(_)) => {
                Predicate::ProposalReturnAccountDoesNotExist
            }
            PhaseOneError::Proposals(InvalidProposals::ReturnAddressWrongNetwork { .. }) => {
                Predicate::ProposalProcedureNetworkIdMismatch
            }
            PhaseOneError::Proposals(InvalidProposals::InvalidPrevGovActionId { .. }) => {
                Predicate::InvalidPrevGovActionId
            }
            PhaseOneError::Proposals(InvalidProposals::InvalidGuardrailsScriptHash { .. }) => {
                Predicate::InvalidGuardrailsScriptHash
            }
            PhaseOneError::Proposals(InvalidProposals::HardforkCantFollow { .. }) => Predicate::ProposalCantFollow,
            PhaseOneError::Proposals(InvalidProposals::MalformedProposal { .. }) => Predicate::MalformedProposal,
            PhaseOneError::Proposals(InvalidProposals::ExpirationEpochTooSmall { .. }) => {
                Predicate::ExpirationEpochTooSmall
            }
            PhaseOneError::VotingProcedures(InvalidVotingProcedures::GovActionsDoNotExist(_)) => {
                Predicate::GovActionsDoNotExist
            }
            PhaseOneError::VotingProcedures(InvalidVotingProcedures::UnknownVoter(_)) => Predicate::VotersDoNotExist,
            PhaseOneError::VotingProcedures(InvalidVotingProcedures::DisallowedVoter(_)) => Predicate::DisallowedVoters,
            PhaseOneError::VotingProcedures(InvalidVotingProcedures::VotingOnExpiredGovAction(_)) => {
                Predicate::VotingOnExpiredGovAction
            }
            PhaseOneError::ValueNotPreserved(_) => Predicate::ValueNotConservedUTxO,
            PhaseOneError::Scripts(InvalidScripts::ExtraneousScriptWitnesses(_)) => {
                Predicate::ExtraneousScriptWitnessesUTXOW
            }
            PhaseOneError::Scripts(InvalidScripts::MissingRequiredScripts(_)) => Predicate::MissingScriptWitnessesUTXOW,
            PhaseOneError::Scripts(InvalidScripts::MissingRequiredDatums { .. }) => Predicate::MissingRequiredDatums,
            PhaseOneError::Scripts(InvalidScripts::MissingRedeemers(_)) => Predicate::MissingRedeemers,
            PhaseOneError::Scripts(InvalidScripts::ExtraneousSupplementalDatums { .. }) => {
                Predicate::NotAllowedSupplementalDatums
            }
            PhaseOneError::Scripts(InvalidScripts::ExtraneousRedeemers(_)) => Predicate::ExtraRedeemers,
            PhaseOneError::Scripts(InvalidScripts::UnspendableInputsNoDatums(_)) => {
                Predicate::UnspendableUTxONoDatumHash
            }
            PhaseOneError::Scripts(InvalidScripts::ScriptWitnessNotValidatingUTXOW(_)) => {
                Predicate::ScriptWitnessNotValidatingUTXOW
            }
            PhaseOneError::Scripts(InvalidScripts::ScriptIntegrityHashMismatch { .. }) => {
                Predicate::PPViewHashesDontMatch
            }
            PhaseOneError::Scripts(InvalidScripts::TooManyExUnits { .. }) => Predicate::TooManyExUnits,
            PhaseOneError::ScriptPreparation(PreparationError::MalformedScriptWitness(_)) => {
                Predicate::MalformedScriptWitnesses
            }
            PhaseOneError::ScriptPreparation(PreparationError::ScriptContextState(ref error)) => {
                let message = error.to_string();
                if message.contains("proposal procedures unsupported") {
                    Predicate::ProposalProceduresFieldNotSupported
                } else if message.contains("voting procedures unsupported") {
                    Predicate::VotingProceduresFieldNotSupported
                } else if message.contains("current treasury value unsupported") {
                    Predicate::CurrentTreasuryFieldNotSupported
                } else if message.contains("treasury donation unsupported") {
                    Predicate::TreasuryDonationFieldNotSupported
                } else if message.contains("inline datums unsupported") {
                    Predicate::InlineDatumsNotSupported
                } else {
                    unreachable!("no predicate mapping yet for {err}")
                }
            }
            PhaseOneError::ScriptPreparation(
                PreparationError::MissingInput(_)
                | PreparationError::TransactionTranslation(_)
                | PreparationError::ScriptDeserialization(_)
                | PreparationError::NonDisjointRefInputs { .. },
            ) => unreachable!("no predicate mapping yet for {err}"),
            PhaseOneError::ScriptPreparation(PreparationError::MissingCostModel(_)) => Predicate::MissingCostModel,
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialInvalidPoolDelegation(ref e)) => match e {
                DelegateError::UnknownSource(_) => Predicate::StakeCredentialInvalidPoolDelegation,
                DelegateError::UnknownTarget(_) => Predicate::DelegateeStakePoolNotRegistered,
                DelegateError::AlreadyResigned => unreachable!("only applicable to CC"),
            },
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialInvalidVoteDelegation(ref e)) => match e {
                DelegateError::UnknownSource(_) => Predicate::StakeCredentialInvalidVoteDelegation,
                DelegateError::UnknownTarget(_) => Predicate::DelegateeDRepNotRegistered,
                DelegateError::AlreadyResigned => unreachable!("only applicable to CC"),
            },
            PhaseOneError::Certificates(InvalidCertificates::CCMemberInvalidDelegation(ref e)) => match e {
                DelegateError::UnknownSource(_) => Predicate::CommitteeIsUnknown,
                DelegateError::AlreadyResigned => Predicate::CommitteeHasPreviouslyResigned,
                DelegateError::UnknownTarget(_) => unreachable!("hot credentials are not pre-registered"),
            },
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialAlreadyRegistered(_)) => {
                Predicate::StakeKeyRegistered
            }
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialHasRewards { .. }) => {
                Predicate::StakeKeyHasNonZeroAccountBalance
            }
            PhaseOneError::Certificates(InvalidCertificates::DRepAlreadyRegistered(_)) => {
                Predicate::DRepAlreadyRegistered
            }
            PhaseOneError::Certificates(InvalidCertificates::DRepInvalidUpdate(_)) => Predicate::DRepNotRegistered,
            PhaseOneError::Certificates(InvalidCertificates::DRepNotRegistered(_)) => Predicate::DRepNotRegistered,
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialNotRegistered(_)) => {
                Predicate::StakeKeyNotRegistered
            }
            PhaseOneError::Certificates(InvalidCertificates::PoolRetirementWrongEpoch { .. }) => {
                Predicate::StakePoolRetirementWrongEpochPOOL
            }
            PhaseOneError::Certificates(InvalidCertificates::StakePoolUnknown(_)) => {
                Predicate::StakePoolNotRegisteredOnKeyPOOL
            }
            PhaseOneError::Certificates(InvalidCertificates::PoolCostTooLow { .. }) => {
                Predicate::StakePoolCostTooLowPOOL
            }
            PhaseOneError::Certificates(InvalidCertificates::PoolMetadataHashTooBig { .. }) => {
                Predicate::PoolMetadataHashTooBig
            }
            PhaseOneError::Certificates(InvalidCertificates::PoolWrongNetwork { .. }) => Predicate::WrongNetworkPOOL,
            PhaseOneError::Collateral(InvalidCollateral::UnknownInput(..)) => Predicate::BadInputsUTxO,
            PhaseOneError::Collateral(InvalidCollateral::InsufficientBalance { .. }) => {
                Predicate::InsufficientCollateral
            }
            PhaseOneError::Collateral(InvalidCollateral::ValueNotConserved(..)) => Predicate::ValueNotConservedUTxO,
            PhaseOneError::Collateral(InvalidCollateral::TooManyInputs { .. }) => Predicate::TooManyCollateralInputs,
            PhaseOneError::Collateral(InvalidCollateral::LockedAtScriptAddress(..)) => Predicate::ScriptsNotPaidUTxO,
            PhaseOneError::Collateral(InvalidCollateral::DeclaredCollateralMismatch { .. }) => {
                Predicate::IncorrectTotalCollateralField
            }
            PhaseOneError::Collateral(InvalidCollateral::NoCollateral) => Predicate::NoCollateralInputs,
            PhaseOneError::Certificates(_)
            | PhaseOneError::Proposals(_)
            | PhaseOneError::VotingProcedures(InvalidVotingProcedures::EraHistory(_)) => {
                unreachable!("no predicate mapping yet for {err}")
            }
        }
    }
}
