use amaru_kernel::{
    Address, AssetName, Bytes, DatumOption, Hash, KeyValuePairs, PolicyId, StakeCredential,
    TransactionInput, from_alonzo_value,
};

use crate::{
    Constr, DEFAULT_TAG, MaybeIndefArray, ToConstrTag, ToPlutusData, constr,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, OutputRef, PlutusData, TimeRange, TransactionId,
        TransactionOutput, Value, Withdrawal,
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
    validity_range: TimeRange,
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

impl ToPlutusData<1> for ScriptContext {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 1, 0, self.tx_info, self.purpose)
    }
}

impl ToPlutusData<1> for TxInfo {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 1, 0, self.inputs, self.outputs, self.fee, self.mint, self.certificates, self.withdrawals.iter().map(|(address, coin)| (constr!(v: 1, 0, address), *coin)).collect::<Vec<_>>(), self.validity_range, self.signatories, self.data, constr!(v:1,0, self.id))
    }
}

impl ToPlutusData<1> for ScriptPurpose {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            ScriptPurpose::Minting(policy_id) => constr!(v: 1, 0, policy_id),
            ScriptPurpose::Spending(input) => constr!(v: 1,1, input),
            ScriptPurpose::Rewarding(stake_credential) => {
                constr!(v: 1, 2, constr!(v: 1, 0, stake_credential))
            }
            ScriptPurpose::Certifying(certificate) => constr!(v: 1, 3, certificate),
        }
    }
}

impl ToPlutusData<1> for Value {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Value::Coin(coin) => PlutusData::Map(KeyValuePairs::Def(vec![(
                <Bytes as ToPlutusData<1>>::to_plutus_data(&Bytes::from(vec![])),
                PlutusData::Map(KeyValuePairs::Def(vec![(
                    <AssetName as ToPlutusData<1>>::to_plutus_data(&AssetName::from(vec![])),
                    <u64 as ToPlutusData<1>>::to_plutus_data(coin),
                )])),
            )])),
            Value::Multiasset(coin, multiasset) => {
                let ada_entry = (
                    <Bytes as ToPlutusData<1>>::to_plutus_data(&Bytes::from(vec![])),
                    PlutusData::Map(KeyValuePairs::Def(vec![(
                        <AssetName as ToPlutusData<1>>::to_plutus_data(&AssetName::from(vec![])),
                        <u64 as ToPlutusData<1>>::to_plutus_data(coin),
                    )])),
                );

                let multiasset_entries = multiasset.iter().map(|(policy_id, assets)| {
                    (
                        <PolicyId as ToPlutusData<1>>::to_plutus_data(policy_id),
                        PlutusData::Map(KeyValuePairs::Def(
                            assets
                                .iter()
                                .map(|(asset, amount)| {
                                    (
                                        <Bytes as ToPlutusData<1>>::to_plutus_data(asset),
                                        <u64 as ToPlutusData<1>>::to_plutus_data(&amount.into()),
                                    )
                                })
                                .collect(),
                        )),
                    )
                });

                PlutusData::Map(KeyValuePairs::Def(
                    std::iter::once(ada_entry)
                        .chain(multiasset_entries)
                        .collect(),
                ))
            }
        }
    }
}

impl ToPlutusData<1> for TransactionInput {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 1, 0, constr!(v: 1, 0, self.transaction_id), self.index)
    }
}

#[allow(clippy::wildcard_enum_match_arm)]
impl ToPlutusData<1> for Certificate {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Certificate::StakeRegistration(stake_credential) => constr!(v: 1, 0, stake_credential),
            Certificate::StakeDeregistration(stake_credential) => {
                constr!(v: 1, 1, stake_credential)
            }
            Certificate::StakeDelegation(stake_credential, hash) => {
                constr!(v: 1, 2, stake_credential, hash)
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
            } => constr!(v: 1, 3, operator, vrf_keyhash),
            Certificate::PoolRetirement(hash, epoch) => constr!(v: 1, 4, hash, epoch),
            certificate => {
                unreachable!("illegal certificate type in v1 script context: {certificate:?}")
            }
        }
    }
}

impl ToPlutusData<1> for OutputRef {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v:1, 0, self.input, self.output)
    }
}

// TODO: remove unwraps and expects here
impl ToPlutusData<1> for TransactionOutput {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            amaru_kernel::PseudoTransactionOutput::Legacy(output) => {
                constr!(v:1, 0, Address::from_bytes(&output.address).unwrap(), from_alonzo_value(&output.amount).expect("illegal alonzo value"), None::<Hash<32>>)
            }
            amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                constr!(v:1, 0, Address::from_bytes(&output.address).unwrap(), output.value, match output.datum_option {
                    Some(DatumOption::Hash(hash)) => Some(hash),
                    _ => None::<Hash<32>>,
                })
            }
        }
    }
}
