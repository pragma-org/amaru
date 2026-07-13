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
            InvalidCertificates, InvalidCollateral, InvalidFees, InvalidInputs, InvalidTransactionMetadata,
            InvalidVKeyWitness, InvalidValidityInterval, InvalidWithdrawals, PhaseOneError,
            outputs::{InvalidOutput, InvalidOutputs},
            proposals::InvalidProposals,
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
    #[serde(deserialize_with = "deserialize_utxo", default)]
    pub(super) utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    #[serde(default)]
    pub(super) pools: BTreeSet<PoolId>,
    #[serde(deserialize_with = "deserialize_accounts", default)]
    pub(super) accounts: BTreeMap<StakeCredential, AccountState>,
    #[serde(deserialize_with = "deserialize_dreps", default)]
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
    DecodingFailure,
    Fail(Predicate),
}

impl<'de> Deserialize<'de> for Expected {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = json::Value::deserialize(d)?;
        match value {
            json::Value::String(s) if s == "Pass" => Ok(Expected::Pass),
            json::Value::Object(ref map) if map.contains_key("decodingFailure") => Ok(Expected::DecodingFailure),
            json::Value::Object(_) => json::from_value(value).map(Expected::Fail).map_err(serde::de::Error::custom),
            json::Value::String(s) => Err(serde::de::Error::custom(format!("expected \"Pass\", got {s:?}"))),
            json::Value::Null | json::Value::Bool(_) | json::Value::Number(_) | json::Value::Array(_) => {
                Err(serde::de::Error::custom("expected \"Pass\", { decodingFailure: ... } or { predicate: ..., ... }"))
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
    IncorrectDepositDELEG,
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
    TreasuryWithdrawalReturnAccountsDoNotExist,
    DelegateeDRepNotRegistered,
    DelegateeStakePoolNotRegistered,
    DRepAlreadyRegistered,
    StakeCredentialInvalidPoolDelegation,
    StakeCredentialInvalidVoteDelegation,
    StakeKeyHasNonZeroAccountBalance,
    StakeKeyRegistered,
    StakePoolRetirementWrongEpochPOOL,
    StakePoolNotRegisteredOnKeyPOOL,
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
            PhaseOneError::InvalidNetworkID { .. } => Predicate::WrongNetworkInTxBody,
            PhaseOneError::TooLarge { .. } => Predicate::MaxTxSizeUTxO,
            PhaseOneError::ValidityInterval(InvalidValidityInterval::OutsideValidityInterval { .. }) => {
                Predicate::OutsideValidityIntervalUTxO
            }
            PhaseOneError::ValidityInterval(InvalidValidityInterval::OutsideForecast(_)) => Predicate::OutsideForecast,
            PhaseOneError::Certificates(InvalidCertificates::IncorrectStakeDeposit { .. }) => {
                Predicate::IncorrectDepositDELEG
            }
            PhaseOneError::Outputs(InvalidOutputs { ref invalid_outputs }) => match invalid_outputs.as_slice() {
                [WithPosition { element: InvalidOutput::TooSmall { .. }, .. }] => Predicate::BabbageOutputTooSmallUTxO,
                [WithPosition { element: InvalidOutput::ValueTooLarge { .. }, .. }] => Predicate::OutputTooBigUTxO,
                [WithPosition { element: InvalidOutput::WrongNetwork { .. }, .. }] => Predicate::WrongNetworkInTxOutput,
                _ => unreachable!("no predicate mapping yet for {err}"),
            },
            PhaseOneError::Proposals(InvalidProposals::TreasuryWithdrawalReturnAccountsDoNotExist(_)) => {
                Predicate::TreasuryWithdrawalReturnAccountsDoNotExist
            }
            PhaseOneError::ValueNotPreserved(_) => Predicate::ValueNotConservedUTxO,
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialInvalidPoolDelegation(ref e)) => match e {
                DelegateError::UnknownSource(_) => Predicate::StakeCredentialInvalidPoolDelegation,
                DelegateError::UnknownTarget(_) => Predicate::DelegateeStakePoolNotRegistered,
            },
            PhaseOneError::Certificates(InvalidCertificates::StakeCredentialInvalidVoteDelegation(ref e)) => match e {
                DelegateError::UnknownSource(_) => Predicate::StakeCredentialInvalidVoteDelegation,
                DelegateError::UnknownTarget(_) => Predicate::DelegateeDRepNotRegistered,
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
            PhaseOneError::Certificates(InvalidCertificates::PoolRetirementWrongEpoch { .. }) => {
                Predicate::StakePoolRetirementWrongEpochPOOL
            }
            PhaseOneError::Certificates(InvalidCertificates::StakePoolUnknown(_)) => {
                Predicate::StakePoolNotRegisteredOnKeyPOOL
            }
            PhaseOneError::Collateral(InvalidCollateral::UnknownInput(..)) => Predicate::BadInputsUTxO,
            PhaseOneError::Collateral(InvalidCollateral::InsufficientBalance { .. }) => {
                Predicate::InsufficientCollateral
            }
            PhaseOneError::Collateral(InvalidCollateral::ValueNotConserved(..)) => Predicate::ValueNotConservedUTxO,
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

#[cfg(test)]
pub(super) mod tx_builder {
    use amaru_kernel::{
        Address, Bytes, Certificate, Hash, Hasher, Lovelace, MemoizedDatum, MemoizedTransactionOutput, MemoizedValue,
        Network, NonEmptySet, NonEmptyVec, Proposal, Set, ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart,
        Slot, StakeCredential, Transaction, TransactionBody, TransactionInput, VKeyWitness, Value, WitnessSet,
        size::KEY, to_cbor,
    };
    use pallas_crypto::key::ed25519;

    const TEST_SK_BYTES: [u8; 32] = [0xAA; 32];

    pub fn test_secret_key() -> ed25519::SecretKey {
        TEST_SK_BYTES.into()
    }

    pub fn test_key_hash() -> Hash<KEY> {
        Hasher::<224>::hash(test_secret_key().public_key().as_ref())
    }

    pub fn test_enterprise_address() -> Address {
        Address::Shelley(ShelleyAddress::new(
            Network::Testnet,
            ShelleyPaymentPart::Key(test_key_hash()),
            ShelleyDelegationPart::Null,
        ))
    }

    pub fn test_credential() -> StakeCredential {
        StakeCredential::AddrKeyhash(test_key_hash())
    }

    fn lovelace_output(address: Address, lovelace: Lovelace) -> MemoizedTransactionOutput {
        MemoizedTransactionOutput::new(
            false,
            address,
            MemoizedValue::new(Value::Coin(lovelace)).unwrap(),
            MemoizedDatum::None,
            None,
        )
    }

    pub fn generate_signed_tx(
        inputs: Vec<TransactionInput>,
        outputs: Vec<MemoizedTransactionOutput>,
        fee: Lovelace,
        certificates: Option<NonEmptySet<Certificate>>,
    ) -> Vec<u8> {
        let sk = test_secret_key();

        let mut body = TransactionBody::default();
        body.inputs = Set::from(inputs);
        body.outputs = outputs;
        body.fee = fee;
        body.certificates = certificates;
        body.validity_interval_start = Some(Slot::from(0));

        let body_bytes = to_cbor(&body);
        let tx_hash: Hash<32> = Hasher::<256>::hash(&body_bytes);

        let signature = sk.sign(tx_hash.as_ref());
        let pk = sk.public_key();

        let vkey_witness = VKeyWitness {
            vkey: Bytes::from(pk.as_ref().to_vec()),
            signature: Bytes::from(signature.as_ref().to_vec()),
        };

        let witnesses =
            WitnessSet { vkeywitness: Some(NonEmptyVec::try_from(vec![vkey_witness]).unwrap()), ..Default::default() };

        let tx = Transaction { body, witnesses, is_expected_valid: true, auxiliary_data: None };

        to_cbor(&tx)
    }

    pub fn generate_signed_tx_with_proposals(
        inputs: Vec<TransactionInput>,
        outputs: Vec<MemoizedTransactionOutput>,
        fee: Lovelace,
        proposals: Option<NonEmptySet<Proposal>>,
    ) -> Vec<u8> {
        let sk = test_secret_key();

        let mut body = TransactionBody::default();
        body.inputs = Set::from(inputs);
        body.outputs = outputs;
        body.fee = fee;
        body.proposals = proposals;
        body.validity_interval_start = Some(Slot::from(0));

        let body_bytes = to_cbor(&body);
        let tx_hash: Hash<32> = Hasher::<256>::hash(&body_bytes);

        let signature = sk.sign(tx_hash.as_ref());
        let pk = sk.public_key();

        let vkey_witness = VKeyWitness {
            vkey: Bytes::from(pk.as_ref().to_vec()),
            signature: Bytes::from(signature.as_ref().to_vec()),
        };

        let witnesses =
            WitnessSet { vkeywitness: Some(NonEmptyVec::try_from(vec![vkey_witness]).unwrap()), ..Default::default() };

        let tx = Transaction { body, witnesses, is_expected_valid: true, auxiliary_data: None };

        to_cbor(&tx)
    }

    pub fn test_reward_account(key_hash: Hash<KEY>) -> Bytes {
        let mut bytes = vec![0xe0u8];
        bytes.extend_from_slice(key_hash.as_ref());
        Bytes::from(bytes)
    }

    pub fn generate_fixture_data(
        input_lovelace: Lovelace,
        output_lovelace: Lovelace,
        fee: Lovelace,
        certificates: Vec<Certificate>,
    ) -> (String, String, String, String) {
        let address = test_enterprise_address();
        let prev_tx_hash = Hash::<32>::new([0xCC; 32]);

        let input = TransactionInput { transaction_id: prev_tx_hash, index: 0 };
        let utxo_output = lovelace_output(address.clone(), input_lovelace);

        let tx_output = lovelace_output(address, output_lovelace);

        let certs = if certificates.is_empty() { None } else { Some(NonEmptySet::try_from(certificates).unwrap()) };

        let tx_bytes = generate_signed_tx(vec![input.clone()], vec![tx_output], fee, certs);

        let input_hex = hex::encode(to_cbor(&input));
        let output_hex = hex::encode(to_cbor(&utxo_output));
        let credential_hex = hex::encode(to_cbor(&test_credential()));
        let tx_hex = hex::encode(&tx_bytes);

        (input_hex, output_hex, credential_hex, tx_hex)
    }
}