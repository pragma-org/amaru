// Copyright 2025 PRAGMA
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
    Address, ComputeHash, EraHistory, Hash, MemoizedDatum, MemoizedTransactionOutput,
    MintedTransactionBody, MintedWitnessSet, PolicyId, Slot, StakeCredential, TransactionInput,
    TransactionInputAdapter,
};
use amaru_slot_arithmetic::EraHistoryError;
use itertools::Itertools;
use thiserror::Error;

use crate::{
    Constr, DEFAULT_TAG, IsKnownPlutusVersion, MaybeIndefArray, PlutusVersion, ToConstrTag,
    ToPlutusData, constr, constr_v1,
    script_context::{
        Certificate, DatumHash, IsPrePlutusVersion3, Mint, OutputRef, PlutusData, RequiredSigners,
        TimeRange, TransactionId, TransactionOutput, Value, Withdrawals,
    },
};

#[derive(Debug, Error)]
pub enum PlutusV1Error {
    #[error("failed to translate inputs: {0}")]
    InputTranslationError(#[from] V1InputTranslationError),
    #[error("reference inputs are not allowed in the v1 script context")]
    ReferenceInputsIncluded,
    #[error("invalid validity range: {0}")]
    InvalidValidityRange(#[from] EraHistoryError),
    #[error("something went wrong: {0}")]
    UnspecifiedError(String),
}

#[derive(Debug, Error)]
pub enum V1InputTranslationError {
    #[error("unknown input: {0}")]
    UnknownInput(TransactionInputAdapter),
    #[error("inline datum included in v1 context: {0}")]
    InlineDatumProvided(TransactionInputAdapter),
    #[error("script ref included in v1 context: {0}")]
    ScriptRefProvided(TransactionInputAdapter),
}

// Reference: https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/V1/Data/Contexts.hs#L148
pub struct TxInfo {
    inputs: Vec<OutputRef>,
    outputs: Vec<TransactionOutput>,
    fee: Value,
    mint: Mint,
    certificates: Vec<Certificate>,
    withdrawals: Withdrawals,
    valid_range: TimeRange,
    signatories: RequiredSigners,
    data: Vec<(DatumHash, PlutusData)>,
    id: TransactionId,
}

impl TxInfo {
    #[allow(clippy::expect_used)]
    pub fn new(
        tx: &MintedTransactionBody<'_>,
        id: &Hash<32>,
        witness_set: &MintedWitnessSet<'_>,
        utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
        era_history: &EraHistory,
        slot: &Slot,
    ) -> Result<Self, PlutusV1Error> {
        if tx.reference_inputs.is_some() {
            return Err(PlutusV1Error::ReferenceInputsIncluded);
        }

        let inputs = Self::translate_inputs(&tx.inputs, utxo)
            .map_err(PlutusV1Error::InputTranslationError)?;

        let outputs = tx
            .outputs
            .iter()
            .map(|output| {
                MemoizedTransactionOutput::try_from(output.clone()).map(TransactionOutput::from)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                PlutusV1Error::UnspecifiedError(format!(
                    "failed to parse transaction output: {}",
                    e
                ))
            })?;

        let certificates = tx
            .certificates
            .clone()
            .map(|set| set.to_vec())
            .unwrap_or_default();

        let withdrawals = tx
            .withdrawals
            .clone()
            .map(Withdrawals::try_from)
            .transpose()
            .map_err(PlutusV1Error::UnspecifiedError)?
            .unwrap_or_default();

        let mint = tx.mint.clone().map(Mint::from).unwrap_or_default();

        let signatories: RequiredSigners = tx
            .required_signers
            .clone()
            .map(RequiredSigners::from)
            .unwrap_or_default();

        let data: Vec<(_, _)> = witness_set
            .plutus_data
            .as_deref()
            .map(|s| {
                s.iter()
                    .cloned()
                    .map(|d| (d.compute_hash(), d.unwrap()))
                    .sorted()
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            inputs,
            outputs,
            fee: tx.fee.into(),
            mint,
            certificates,
            withdrawals,
            valid_range: TimeRange::new(
                tx.validity_interval_start.map(Slot::from),
                tx.ttl.map(Slot::from),
                slot,
                era_history,
            )
            .map_err(PlutusV1Error::InvalidValidityRange)?,
            signatories,
            data,
            id: *id,
        })
    }

    fn translate_inputs(
        inputs: &[TransactionInput],
        utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    ) -> Result<Vec<OutputRef>, V1InputTranslationError> {
        inputs
            .iter()
            .sorted()
            .filter_map(|input| {
                let utxo = match utxo.get(input) {
                    Some(resolved) => resolved,
                    None => {
                        return Some(Err(V1InputTranslationError::UnknownInput(
                            input.clone().into(),
                        )));
                    }
                };

                // In v1, we filter out Byron addresses due to a mistake in Alonzo
                // https://github.com/IntersectMBO/cardano-ledger/blob/59b52bb31c76a4a805e18860f68f549ec9022b14/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Plutus/TxInfo.hs#L111-L112
                match &utxo.address {
                    Address::Byron(_) => return None,
                    Address::Stake(_) => unreachable!("stake address in UTxO"),
                    _ => (),
                };

                if let MemoizedDatum::Inline(_) = utxo.datum {
                    return Some(Err(V1InputTranslationError::InlineDatumProvided(
                        input.clone().into(),
                    )));
                }

                if utxo.script.is_some() {
                    return Some(Err(V1InputTranslationError::ScriptRefProvided(
                        input.clone().into(),
                    )));
                }

                Some(Ok(OutputRef {
                    input: input.clone(),
                    output: utxo.clone().into(),
                }))
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

#[derive(Clone)]
pub enum ScriptPurpose {
    Minting(PolicyId),
    Spending(TransactionInput),
    Rewarding(StakeCredential),
    Certifying(Certificate),
}

pub struct ScriptContext {
    tx_info: TxInfo,
    purpose: ScriptPurpose,
}

impl<const V: u8> ToPlutusData<V> for ScriptContext
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    fn to_plutus_data(&self) -> PlutusData {
        constr_v1!(0, [self.tx_info, self.purpose])
    }
}

impl ToPlutusData<1> for TxInfo {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v1!(
            0,
            [
                self.inputs,
                self.outputs,
                self.fee,
                self.mint,
                self.certificates,
                self.withdrawals,
                self.valid_range,
                self.signatories,
                self.data,
                constr_v1!(0, [self.id])
            ]
        )
    }
}

impl<const V: u8> ToPlutusData<V> for ScriptPurpose
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            ScriptPurpose::Minting(policy_id) => constr_v1!(0, [policy_id]),
            ScriptPurpose::Spending(input) => constr_v1!(1, [input]),
            ScriptPurpose::Rewarding(stake_credential) => {
                constr_v1!(2, [constr_v1!(0, [stake_credential])])
            }
            ScriptPurpose::Certifying(certificate) => constr_v1!(3, [certificate]),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for Value
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    fn to_plutus_data(&self) -> PlutusData {
        self.0.to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for TransactionInput
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: V, 0, [constr!(v: V, 0, [self.transaction_id]), self.index])
    }
}

#[allow(clippy::wildcard_enum_match_arm)]
impl<const V: u8> ToPlutusData<V> for Certificate
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Certificate::StakeRegistration(stake_credential) => {
                constr!(v: V, 0, [stake_credential])
            }
            Certificate::StakeDeregistration(stake_credential) => {
                constr!(v: V, 1, [stake_credential])
            }
            Certificate::StakeDelegation(stake_credential, hash) => {
                constr!(v: V, 2, [stake_credential, hash])
            }
            Certificate::PoolRegistration {
                operator,
                vrf_keyhash,
                pledge: _,
                cost: _,
                margin: _,
                reward_account: _,
                pool_owners: _,
                relays: _,
                pool_metadata: _,
            } => constr!(v: V, 3, [operator, vrf_keyhash]),
            Certificate::PoolRetirement(hash, epoch) => constr!(v: V, 4, [hash, epoch]),
            certificate => {
                unreachable!("illegal certificate type in v{V:?} script context: {certificate:?}")
            }
        }
    }
}

impl ToPlutusData<1> for OutputRef {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v1!(0, [self.input, self.output])
    }
}

impl ToPlutusData<1> for TransactionOutput {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn to_plutus_data(&self) -> PlutusData {
        constr_v1!(
            0,
            [
                self.address,
                self.value,
                match self.datum {
                    MemoizedDatum::Hash(hash) => Some(hash),
                    _ => None::<Hash<32>>,
                },
            ]
        )
    }
}

impl ToPlutusData<1> for Mint {
    fn to_plutus_data(&self) -> PlutusData {
        // In V1, we need to provide the zero ADA asset as well
        let mut mint = self
            .0
            .iter()
            .map(|(policy, multiasset)| (policy.to_vec(), multiasset))
            .collect::<BTreeMap<_, _>>();

        let ada_bundle = BTreeMap::from([(vec![].into(), 0)]);
        mint.insert(vec![], &ada_bundle);

        <BTreeMap<_, _> as ToPlutusData<1>>::to_plutus_data(&mint)
    }
}

impl ToPlutusData<1> for RequiredSigners {
    fn to_plutus_data(&self) -> PlutusData {
        let vec = self.0.iter().collect::<Vec<_>>();

        <Vec<_> as ToPlutusData<1>>::to_plutus_data(&vec)
    }
}

impl ToPlutusData<1> for Withdrawals {
    fn to_plutus_data(&self) -> PlutusData {
        <Vec<_> as ToPlutusData<1>>::to_plutus_data(
            &self
                .0
                .iter()
                .map(|(address, coin)| (constr_v1!(0, [address]), *coin))
                .collect::<Vec<_>>(),
        )
    }
}
