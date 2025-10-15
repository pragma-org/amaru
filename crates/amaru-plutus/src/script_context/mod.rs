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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    AddrKeyhash, Address, Certificate, DatumHash, EraHistory, Hash, KeyValuePairs, Lovelace,
    MemoizedDatum, MemoizedScript, MemoizedTransactionOutput, Mint as KernelMint, PlutusData,
    PolicyId, Redeemer, RequiredSigners as KernelRequiredSigners, StakeAddress, TransactionId,
    TransactionInput, Value as KernelValue, Voter, Withdrawal,
};
use amaru_kernel::{AssetName, Slot};

use amaru_slot_arithmetic::{EraHistoryError, TimeMs};

pub mod v1;
pub mod v2;
pub mod v3;

pub trait IsPrePlutusVersion3 {}
impl IsPrePlutusVersion3 for PlutusVersion<1> {}
impl IsPrePlutusVersion3 for PlutusVersion<2> {}

pub use v1::ScriptContext as ScriptContextV1;
pub use v1::TxInfo as TxInfoV1;
pub use v2::ScriptContext as ScriptContextV2;
pub use v2::TxInfo as TxInfoV2;
pub use v3::TxInfo as TxInfoV3;

use crate::PlutusVersion;

pub struct OutputRef {
    pub input: TransactionInput,
    pub output: TransactionOutput,
}

pub struct TimeRange {
    pub lower_bound: Option<TimeMs>,
    pub upper_bound: Option<TimeMs>,
}

impl TimeRange {
    pub fn new(
        valid_from_slot: Option<Slot>,
        valid_to_slot: Option<Slot>,
        tip: &Slot,
        era_history: &EraHistory,
    ) -> Result<Self, EraHistoryError> {
        let lower_bound = valid_from_slot
            .map(|slot| era_history.slot_to_relative_time(slot, *tip))
            .transpose()?;
        let upper_bound = valid_to_slot
            .map(|slot| era_history.slot_to_relative_time(slot, *tip))
            .transpose()?;

        Ok(Self {
            lower_bound,
            upper_bound,
        })
    }
}

// This is a variant of `PolicyId` that makes it easier to work with
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurrencySymbol {
    Ada,
    Native(Hash<28>),
}

impl From<Hash<28>> for CurrencySymbol {
    fn from(value: Hash<28>) -> Self {
        Self::Native(value)
    }
}

#[derive(Clone)]
pub struct Value(pub BTreeMap<CurrencySymbol, BTreeMap<AssetName, u64>>);

impl From<KernelValue> for Value {
    fn from(value: KernelValue) -> Self {
        let assets = match value {
            KernelValue::Coin(coin) => {
                BTreeMap::from([(CurrencySymbol::Ada, BTreeMap::from([(vec![].into(), coin)]))])
            }
            KernelValue::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(CurrencySymbol::Ada, BTreeMap::from([(vec![].into(), coin)]));
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
            CurrencySymbol::Ada,
            BTreeMap::from([(vec![].into(), coin)]),
        )]))
    }
}

impl Value {
    pub fn ada(&self) -> Option<u64> {
        self.0.get(&CurrencySymbol::Ada).and_then(|asset_bundle| {
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

#[derive(Clone)]
pub struct TransactionOutput {
    pub is_legacy: bool,
    pub address: Address,
    pub value: Value,
    pub datum: MemoizedDatum,
    pub script: Option<MemoizedScript>,
}

impl From<MemoizedTransactionOutput> for TransactionOutput {
    fn from(output: MemoizedTransactionOutput) -> Self {
        Self {
            is_legacy: output.is_legacy,
            address: output.address,
            value: output.value.into(),
            datum: output.datum,
            script: output.script,
        }
    }
}

#[derive(Debug, Default)]
pub struct Mint(pub BTreeMap<Hash<28>, BTreeMap<AssetName, i64>>);

impl From<KernelMint> for Mint {
    fn from(value: KernelMint) -> Self {
        let mints: BTreeMap<Hash<28>, BTreeMap<AssetName, i64>> = value
            .into_iter()
            .map(|(policy, multiasset)| {
                (
                    policy,
                    multiasset
                        .into_iter()
                        .map(|(asset_name, amount)| (asset_name, amount.into()))
                        .collect(),
                )
            })
            .collect();

        Self(mints)
    }
}

#[derive(Default)]
pub struct RequiredSigners(pub BTreeSet<AddrKeyhash>);

impl From<KernelRequiredSigners> for RequiredSigners {
    fn from(value: KernelRequiredSigners) -> Self {
        Self(value.to_vec().into_iter().collect())
    }
}
