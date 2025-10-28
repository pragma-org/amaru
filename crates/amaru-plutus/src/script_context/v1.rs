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
    Address, EraHistory, Hash, MemoizedDatum, MemoizedTransactionOutput, MintedTransactionBody,
    MintedWitnessSet, PolicyId, Redeemer, ScriptPurpose as RedeemerTag, Slot, StakeCredential,
    TransactionInput, TransactionInputAdapter, network::NetworkName, normalize_redeemers,
};
use amaru_slot_arithmetic::EraHistoryError;
use itertools::Itertools;
use thiserror::Error;

use crate::{
    IsKnownPlutusVersion, PlutusVersion, ToPlutusData, constr, constr_v1,
    script_context::{
        Certificate, Datums, IsPrePlutusVersion3, Mint, OutputRef, PlutusData, RequiredSigners,
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
    #[error("invalid redeemer at index {0}")]
    InvalidRedeemer(usize),
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
    data: Datums,
    redeemers: Redeemers,
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
        network: NetworkName,
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

        let datums = witness_set
            .plutus_data
            .clone()
            .map(Datums::from)
            .unwrap_or_default();

        let redeemers = witness_set
            .redeemer
            .clone()
            .map(|redeemers| {
                normalize_redeemers(redeemers.unwrap())
                    .into_iter()
                    .enumerate()
                    .map(|(ix, redeemer)| {
                        let purpose = ScriptPurpose::builder(
                            &redeemer,
                            &inputs[..],
                            &mint,
                            &withdrawals,
                            &certificates,
                        )
                        .map_err(|_| PlutusV1Error::InvalidRedeemer(ix))?;

                        Ok((purpose, redeemer))
                    })
                    .collect::<Result<Vec<(ScriptPurpose, Redeemer)>, PlutusV1Error>>()
            })
            .transpose()?
            .unwrap_or_default();

        let redeemers = Redeemers(redeemers);

        let valid_range = TimeRange::new(
            tx.validity_interval_start.map(Slot::from),
            tx.ttl.map(Slot::from),
            slot,
            era_history,
            network,
        )
        .map_err(PlutusV1Error::InvalidValidityRange)?;
        Ok(Self {
            inputs,
            outputs,
            fee: tx.fee.into(),
            mint,
            certificates,
            withdrawals,
            valid_range,
            signatories,
            redeemers,
            data: datums,
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

// FIXME: This should probably be a BTreeMap
pub struct Redeemers(pub Vec<(ScriptPurpose, Redeemer)>);

impl ScriptPurpose {
    #[allow(clippy::result_unit_err)]
    pub fn builder(
        redeemer: &Redeemer,
        inputs: &[OutputRef],
        mint: &Mint,
        withdrawals: &Withdrawals,
        certs: &[Certificate],
    ) -> Result<Self, ()> {
        let index = redeemer.index as usize;
        match redeemer.tag {
            RedeemerTag::Spend => inputs
                .get(index)
                .map(|output_ref| ScriptPurpose::Spending(output_ref.input.clone())),
            RedeemerTag::Mint => mint
                .0
                .keys()
                .nth(index)
                .map(|policy| ScriptPurpose::Minting(*policy)),
            RedeemerTag::Reward => withdrawals.0.keys().nth(index).map(|stake| {
                ScriptPurpose::Rewarding(match stake.0.payload() {
                    amaru_kernel::StakePayload::Stake(hash) => StakeCredential::AddrKeyhash(*hash),
                    amaru_kernel::StakePayload::Script(hash) => StakeCredential::ScriptHash(*hash),
                })
            }),
            RedeemerTag::Cert => certs
                .get(index)
                .map(|cert| ScriptPurpose::Certifying(cert.clone())),
            RedeemerTag::Vote | RedeemerTag::Propose => None,
        }
        .ok_or(())
    }
}

pub struct ScriptContext {
    tx_info: TxInfo,
    purpose: ScriptPurpose,
}

impl ScriptContext {
    pub fn new(tx_info: TxInfo, redeemer: &Redeemer) -> Option<Self> {
        let purpose = tx_info
            .redeemers
            .0
            .iter()
            .find_map(|(purpose, tx_redeemer)| {
                if redeemer.tag == tx_redeemer.tag && redeemer.index == tx_redeemer.index {
                    Some(purpose.clone())
                } else {
                    None
                }
            });

        purpose.map(|purpose| ScriptContext { tx_info, purpose })
    }
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
        constr!(0, [constr!(0, [self.transaction_id]), self.index])
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
                constr!(0, [stake_credential])
            }
            Certificate::StakeDeregistration(stake_credential) => {
                constr!(1, [stake_credential])
            }
            Certificate::StakeDelegation(stake_credential, hash) => {
                constr!(2, [stake_credential, hash])
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
            } => constr!(3, [operator, vrf_keyhash]),
            Certificate::PoolRetirement(hash, epoch) => constr!(4, [hash, epoch]),
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

impl ToPlutusData<1> for Datums {
    fn to_plutus_data(&self) -> PlutusData {
        <Vec<_> as ToPlutusData<1>>::to_plutus_data(&self.0.iter().collect::<Vec<_>>())
    }
}
