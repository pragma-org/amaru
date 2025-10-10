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

use std::{collections::BTreeMap, ops::Deref};

use amaru_kernel::{
    Address, AssetName, Bytes, Hash, IsSortable, KeyValuePairs, MemoizedDatum,
    MintedTransactionBody, NonEmptyKeyValuePairs, PolicyId, StakeCredential, TransactionInput,
    TransactionInputAdapter,
};
use itertools::Itertools;
use thiserror::Error;

use crate::{
    Constr, DEFAULT_TAG, IsKnownPlutusVersion, MaybeIndefArray, PlutusVersion, ToConstrTag,
    ToPlutusData, constr, constr_v1,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, IsPrePlutusVersion3, OutputRef, PlutusData, TimeRange,
        TransactionId, TransactionOutput, Value, Withdrawal,
    },
};

#[derive(Debug, Error)]
pub enum PlutusV1Error {
    #[error("failed to translate inputs: {0}")]
    InputTranslationError(#[from] V1InputTranslationError),
    #[error("reference inputs are not allowed in the v1 script context")]
    ReferenceInputsIncluded,
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
    mint: Value,
    certificates: Vec<Certificate>,
    withdrawals: Vec<Withdrawal>,
    valid_range: TimeRange,
    signatories: Vec<AddrKeyhash>,
    data: Vec<(DatumHash, PlutusData)>,
    id: TransactionId,
}

impl TxInfo {
    pub fn new(
        tx: &MintedTransactionBody<'_>,
        utxo: BTreeMap<TransactionInput, TransactionOutput>,
    ) -> Result<Self, PlutusV1Error> {
        if tx.reference_inputs.is_some() {
            return Err(PlutusV1Error::ReferenceInputsIncluded);
        }

        let inputs = Self::translate_inputs(&*tx.inputs, utxo)
            .map_err(PlutusV1Error::InputTranslationError)?;
        let certificates = tx
            .certificates
            .clone()
            .map(|set| set.to_vec())
            .unwrap_or_default();
        let withdrawals = tx.withdrawals.as_ref().map(|withdrawals| {
            withdrawals
                .iter()
                .map(|(reward_account, coin)| {
                    let address = Address::from_bytes(&reward_account)
                        .expect("invalid address bytes in withdrawal");
                    if let Address::Stake(reward_account) = address {
                        (reward_account, *coin)
                    } else {
                        unreachable!("invalid reward address in withdrawals")
                    }
                })
                .sorted_by(|(a, _), (b, _)| a.sort(b))
                .collect::<Vec<Withdrawal>>()
        });

        todo!()
    }

    fn translate_inputs(
        inputs: &[TransactionInput],
        utxo: BTreeMap<TransactionInput, TransactionOutput>,
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

                if let Some(_) = utxo.script {
                    return Some(Err(V1InputTranslationError::ScriptRefProvided(
                        input.clone().into(),
                    )));
                }

                Some(Ok(OutputRef {
                    input: input.clone(),
                    // TODO: The value in our output has to be lexicographically sorted
                    output: sorted_utxo(utxo.clone()),
                }))
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

fn sorted_utxo(utxo: TransactionOutput) -> TransactionOutput {
    TransactionOutput {
        is_legacy: utxo.is_legacy,
        address: utxo.address,
        value: sort_value(utxo.value),
        datum: utxo.datum,
        script: utxo.script,
    }
}

// TODO: this is grabbed straight from Aiken.
// This should be rethought to avoid cloning
fn sort_value(value: Value) -> Value {
    match value {
        Value::Coin(_) => value.clone(),
        Value::Multiasset(coin, multiasset) => Value::Multiasset(
            coin,
            NonEmptyKeyValuePairs::Indef(
                multiasset
                    .deref()
                    .into_iter()
                    .sorted()
                    .map(|(policy_id, asset_bundle)| {
                        (
                            *policy_id,
                            NonEmptyKeyValuePairs::Indef(
                                asset_bundle.deref().iter().sorted().cloned().collect(),
                            ),
                        )
                    })
                    .collect(),
            ),
        ),
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
                self.withdrawals
                    .iter()
                    .map(|(address, coin)| (constr_v1!(0, [address]), *coin))
                    .collect::<Vec<_>>(),
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
        let ada_entry = |coin: &u64| -> (PlutusData, PlutusData) {
            (
                <Bytes as ToPlutusData<V>>::to_plutus_data(&Bytes::from(vec![])),
                PlutusData::Map(KeyValuePairs::Def(vec![(
                    <AssetName as ToPlutusData<V>>::to_plutus_data(&AssetName::from(vec![])),
                    <u64 as ToPlutusData<V>>::to_plutus_data(coin),
                )])),
            )
        };
        let entries = match self {
            Value::Coin(coin) => vec![ada_entry(coin)],
            Value::Multiasset(coin, multiasset) => {
                let multiasset_entries = multiasset.iter().map(|(policy_id, assets)| {
                    (
                        <PolicyId as ToPlutusData<V>>::to_plutus_data(policy_id),
                        PlutusData::Map(KeyValuePairs::Def(
                            assets
                                .iter()
                                .map(|(asset, amount)| {
                                    (
                                        <Bytes as ToPlutusData<V>>::to_plutus_data(asset),
                                        <u64 as ToPlutusData<V>>::to_plutus_data(&amount.into()),
                                    )
                                })
                                .collect(),
                        )),
                    )
                });
                std::iter::once(ada_entry(coin))
                    .chain(multiasset_entries)
                    .collect()
            }
        };

        PlutusData::Map(KeyValuePairs::Def(entries))
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

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl ToPlutusData<1> for TransactionOutput {
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
