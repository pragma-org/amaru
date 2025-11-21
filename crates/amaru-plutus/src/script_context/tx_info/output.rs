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

use std::{borrow::Borrow, collections::BTreeMap, ops::Deref};

use amaru_kernel::{
    Address, AddressError, AlonzoValue, AssetName, Bytes, CborWrap, ComputeHash, DatumHash, Hash,
    KeepRaw, KeyValuePairs, Lovelace, MemoizedDatum, MemoizedPlutusData, MemoizedScript,
    MemoizedTransactionOutput, MintedDatumOption, MintedScriptRef, MintedTransactionOutput,
    NativeScript, NonEmptySet, PlutusData, PlutusScript, PseudoScript, TransactionInput,
    arc_mapped::ArcMapped,
};
use thiserror::Error;

/// A resolved input which includes the output it references.
#[doc(hidden)]
#[derive(Debug)]
pub struct OutputRef {
    pub input: TransactionInput,
    pub output: TransactionOutput,
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct TransactionOutput {
    pub is_legacy: bool,
    pub address: Address,
    pub value: Value,
    pub datum: DatumOption,
    pub script: Option<Script>,
}

impl From<MemoizedTransactionOutput> for TransactionOutput {
    fn from(output: MemoizedTransactionOutput) -> Self {
        Self {
            is_legacy: output.is_legacy,
            address: output.address,
            value: output.value.into(),
            datum: output.datum.into(),
            script: output.script.map(Script::from),
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Error)]
pub enum TransactionOutputError {
    #[error("invalid address: {0}")]
    InvalidAddress(#[from] AddressError),
    #[error("invalid value: {0}")]
    InvalidValue(#[from] AlonzoValueError),
}

impl TryFrom<MintedTransactionOutput<'_>> for TransactionOutput {
    type Error = TransactionOutputError;

    fn try_from(output: MintedTransactionOutput<'_>) -> Result<TransactionOutput, Self::Error> {
        match output {
            MintedTransactionOutput::Legacy(output) => Ok(TransactionOutput {
                is_legacy: true,
                address: Address::from_bytes(&output.address)?,
                value: (&output.amount).try_into()?,
                datum: output.datum_hash.as_ref().into(),
                script: None,
            }),
            MintedTransactionOutput::PostAlonzo(output) => Ok(TransactionOutput {
                is_legacy: false,
                address: Address::from_bytes(&output.address)?,
                value: (&output.value).into(),
                script: output.script_ref.map(Script::from),
                datum: output.datum_option.into(),
            }),
        }
    }
}

/// An identifier for a currency in a [`Value`]
///
/// See [`Value`] for more
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurrencySymbol {
    Lovelace,
    Native(Hash<28>),
}

impl From<Hash<28>> for CurrencySymbol {
    fn from(value: Hash<28>) -> Self {
        Self::Native(value)
    }
}

/// A representation of `Value` used in Plutus
///
/// The ledger's `Value` contains both a `Coin` and, optionally, a `Multiasset`.
/// In Plutus, this is simply a single map, with an empty bytestring representing lovelace
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct Value(pub BTreeMap<CurrencySymbol, BTreeMap<AssetName, u64>>);

impl From<&amaru_kernel::Value> for Value {
    fn from(value: &amaru_kernel::Value) -> Self {
        let assets = match value {
            amaru_kernel::Value::Coin(coin) => BTreeMap::from([(
                CurrencySymbol::Lovelace,
                BTreeMap::from([(Bytes::from(vec![]), *coin)]),
            )]),
            amaru_kernel::Value::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(
                    CurrencySymbol::Lovelace,
                    BTreeMap::from([(Bytes::from(vec![]), *coin)]),
                );
                multiasset.iter().for_each(|(policy_id, asset_bundle)| {
                    map.insert(
                        CurrencySymbol::Native(*policy_id),
                        asset_bundle
                            .iter()
                            .map(|(asset_name, amount)| (asset_name.clone(), amount.into()))
                            .collect(),
                    );
                });

                map
            }
        };

        Self(assets)
    }
}

impl From<amaru_kernel::Value> for Value {
    fn from(value: amaru_kernel::Value) -> Self {
        let assets = match value {
            amaru_kernel::Value::Coin(coin) => BTreeMap::from([(
                CurrencySymbol::Lovelace,
                BTreeMap::from([(Bytes::from(vec![]), coin)]),
            )]),
            amaru_kernel::Value::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(
                    CurrencySymbol::Lovelace,
                    BTreeMap::from([(Bytes::from(vec![]), coin)]),
                );
                multiasset
                    .into_iter()
                    .for_each(|(policy_id, asset_bundle)| {
                        map.insert(
                            CurrencySymbol::Native(policy_id),
                            asset_bundle
                                .into_iter()
                                .map(|(asset_name, amount)| (asset_name, amount.into()))
                                .collect(),
                        );
                    });

                map
            }
        };

        Self(assets)
    }
}

impl From<Lovelace> for Value {
    fn from(coin: Lovelace) -> Self {
        Self(BTreeMap::from([(
            CurrencySymbol::Lovelace,
            BTreeMap::from([(Bytes::from(vec![]), coin)]),
        )]))
    }
}

#[doc(hidden)]
#[derive(Debug, Error)]
pub enum AlonzoValueError {
    #[error("invalid quantity: {0}")]
    InvalidQuantity(u64),
}
impl TryFrom<&AlonzoValue> for Value {
    type Error = AlonzoValueError;

    fn try_from(value: &AlonzoValue) -> Result<Self, Self::Error> {
        let from_tokens = |tokens: &KeyValuePairs<AssetName, Lovelace>| {
            (*tokens)
                .iter()
                .map(|(asset_name, quantity)| {
                    if *quantity > 0 {
                        Ok((asset_name.clone(), *quantity))
                    } else {
                        Err(AlonzoValueError::InvalidQuantity(*quantity))
                    }
                })
                .collect::<Result<BTreeMap<_, _>, Self::Error>>()
        };

        match value {
            AlonzoValue::Coin(coin) => Ok((*coin).into()),
            AlonzoValue::Multiasset(coin, multiasset) if multiasset.is_empty() => {
                Ok((*coin).into())
            }
            AlonzoValue::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(
                    CurrencySymbol::Lovelace,
                    BTreeMap::from([(Bytes::from(vec![]), *coin)]),
                );

                for (policy_id, tokens) in multiasset.deref() {
                    map.insert(CurrencySymbol::Native(*policy_id), from_tokens(tokens)?);
                }

                Ok(Self(map))
            }
        }
    }
}

impl Value {
    pub fn ada(&self) -> Option<u64> {
        self.0
            .get(&CurrencySymbol::Lovelace)
            .and_then(|asset_bundle| {
                asset_bundle.iter().find_map(|(name, amount)| {
                    if name.is_empty() && amount != &0 {
                        Some(*amount)
                    } else {
                        None
                    }
                })
            })
    }
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub enum Script {
    Native(NativeScript),
    PlutusV1(PlutusScript<1>),
    PlutusV2(PlutusScript<2>),
    PlutusV3(PlutusScript<3>),
}

impl From<CborWrap<MintedScriptRef<'_>>> for Script {
    fn from(value: CborWrap<MintedScriptRef<'_>>) -> Self {
        match value.0 {
            PseudoScript::NativeScript(script) => Script::Native(script.unwrap()),
            PseudoScript::PlutusV1Script(script) => Script::PlutusV1(script),
            PseudoScript::PlutusV2Script(script) => Script::PlutusV2(script),
            PseudoScript::PlutusV3Script(script) => Script::PlutusV3(script),
        }
    }
}

impl From<MemoizedScript> for Script {
    fn from(value: MemoizedScript) -> Self {
        match value {
            PseudoScript::NativeScript(script) => Script::Native(script.as_ref().clone()),
            PseudoScript::PlutusV1Script(script) => Script::PlutusV1(script),
            PseudoScript::PlutusV2Script(script) => Script::PlutusV2(script),
            PseudoScript::PlutusV3Script(script) => Script::PlutusV3(script),
        }
    }
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub enum DatumOption {
    None,
    Hash(DatumHash),
    Inline(PlutusData),
}

impl From<MemoizedDatum> for DatumOption {
    fn from(value: MemoizedDatum) -> Self {
        match value {
            MemoizedDatum::None => Self::None,
            MemoizedDatum::Hash(hash) => Self::Hash(hash),
            MemoizedDatum::Inline(data) => Self::Inline(data.as_ref().clone()),
        }
    }
}

impl From<Option<MintedDatumOption<'_>>> for DatumOption {
    fn from(value: Option<MintedDatumOption<'_>>) -> Self {
        match value {
            None => Self::None,
            Some(MintedDatumOption::Hash(hash)) => Self::Hash(hash),
            Some(MintedDatumOption::Data(data)) => Self::Inline(data.0.unwrap()),
        }
    }
}

impl From<Option<&Hash<32>>> for DatumOption {
    fn from(value: Option<&Hash<32>>) -> Self {
        match value {
            Some(hash) => Self::Hash(*hash),
            None => Self::None,
        }
    }
}

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct Datums(pub BTreeMap<DatumHash, PlutusData>);

impl From<NonEmptySet<KeepRaw<'_, PlutusData>>> for Datums {
    fn from(plutus_data: NonEmptySet<KeepRaw<'_, PlutusData>>) -> Self {
        Self(
            plutus_data
                .to_vec()
                .into_iter()
                .map(|data| (data.compute_hash(), data.unwrap()))
                .collect(),
        )
    }
}
impl From<&BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>>>
    for Datums
{
    fn from(
        value: &BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>>,
    ) -> Self {
        Self(
            value
                .iter()
                .map(|(hash, mapped)| {
                    let data: &MemoizedPlutusData = mapped.borrow();
                    (*hash, data.as_ref().clone())
                })
                .collect(),
        )
    }
}
