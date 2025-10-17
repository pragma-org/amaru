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

use amaru_kernel::{
    Address, AssetName, Bytes, DatumOption, Hash, KeyValuePairs, PolicyId, StakeCredential,
    TransactionInput, from_alonzo_value,
};

use crate::{
    Constr, DEFAULT_TAG, IsKnownPlutusVersion, MaybeIndefArray, PlutusVersion, ToConstrTag,
    ToPlutusData, constr, constr_v1,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, IsPrePlutusVersion3, OutputRef, PlutusData, TimeRange,
        TransactionId, TransactionOutput, Value, Withdrawal,
    },
};

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
        match self {
            amaru_kernel::PseudoTransactionOutput::Legacy(output) => {
                constr_v1!(
                    0,
                    [
                        Address::from_bytes(&output.address).unwrap(),
                        from_alonzo_value(output.amount.clone()).expect("illegal alonzo value"),
                        output.datum_hash.map(DatumOption::Hash)
                    ]
                )
            }
            amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                constr_v1!(
                    0,
                    [
                        Address::from_bytes(&output.address).unwrap(),
                        output.value,
                        match output.datum_option {
                            Some(DatumOption::Hash(hash)) => Some(hash),
                            _ => None::<Hash<32>>,
                        }
                    ]
                )
            }
        }
    }
}
