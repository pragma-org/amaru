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

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::network::NetworkName;
use amaru_kernel::protocol_parameters::GlobalParameters;
use amaru_kernel::{
    AddrKeyhash, Address, Certificate, ComputeHash, DatumHash, EraHistory, Hash, KeepRaw,
    KeyValuePairs, Lovelace, MemoizedDatum, MemoizedScript, MemoizedTransactionOutput,
    Mint as KernelMint, Network, NonEmptyKeyValuePairs, NonEmptySet, PlutusData, ProposalIdAdapter,
    Redeemer, RequiredSigners as KernelRequiredSigners, RewardAccount,
    StakeAddress as KernelStakeAddress, StakePayload, TransactionId, TransactionInput,
    Value as KernelValue, Vote, Voter, VotingProcedures,
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

pub type Utxos = BTreeMap<TransactionInput, MemoizedTransactionOutput>;

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
        network: NetworkName,
    ) -> Result<Self, EraHistoryError> {
        let parameters: &GlobalParameters = network.into();
        let lower_bound = valid_from_slot
            .map(|slot| era_history.slot_to_posix_time(slot, *tip, parameters.system_start.into()))
            .transpose()?;
        let upper_bound = valid_to_slot
            .map(|slot| era_history.slot_to_posix_time(slot, *tip, parameters.system_start.into()))
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

#[derive(Default)]
pub struct Withdrawals(pub BTreeMap<StakeAddress, Lovelace>);

#[derive(Clone)]
pub struct StakeAddress(KernelStakeAddress);

impl From<StakeAddress> for KernelStakeAddress {
    fn from(value: StakeAddress) -> Self {
        value.0
    }
}

// Reference for this ordering is
// https://github.com/aiken-lang/aiken/blob/a8c032935dbaf4a1140e9d8be5c270acd32c9e8c/crates/uplc/src/tx/script_context.rs#L1112
impl Ord for StakeAddress {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn network_tag(network: Network) -> u8 {
            match network {
                Network::Testnet => 0,
                Network::Mainnet => 1,
                Network::Other(tag) => tag,
            }
        }

        if self.0.network() != other.0.network() {
            return network_tag(self.0.network()).cmp(&network_tag(other.0.network()));
        }

        match (self.0.payload(), other.0.payload()) {
            (StakePayload::Script(..), StakePayload::Stake(..)) => Ordering::Less,
            (StakePayload::Stake(..), StakePayload::Script(..)) => Ordering::Greater,
            (StakePayload::Script(hash_a), StakePayload::Script(hash_b)) => hash_a.cmp(hash_b),
            (StakePayload::Stake(hash_a), StakePayload::Stake(hash_b)) => hash_a.cmp(hash_b),
        }
    }
}

impl PartialOrd for StakeAddress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for StakeAddress {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for StakeAddress {}

impl TryFrom<NonEmptyKeyValuePairs<RewardAccount, Lovelace>> for Withdrawals {
    type Error = String;

    fn try_from(
        value: NonEmptyKeyValuePairs<RewardAccount, Lovelace>,
    ) -> Result<Self, Self::Error> {
        let withdrawals = value
            .iter()
            .map(|(reward_account, coin)| {
                let address = Address::from_bytes(reward_account)
                    .map_err(|e| format!("failed to decode reward account: {}", e))?;

                if let Address::Stake(reward_account) = address {
                    Ok((StakeAddress(reward_account), *coin))
                } else {
                    Err("invalid address type in withdrawals".into())
                }
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;

        Ok(Self(withdrawals))
    }
}

#[derive(Default)]
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

// FIXME: This should probably be a BTreeMap
pub struct Redeemers<T>(pub Vec<(T, Redeemer)>);

#[derive(Default)]
pub struct Votes(pub BTreeMap<Voter, BTreeMap<ProposalIdAdapter, Vote>>);

impl From<VotingProcedures> for Votes {
    fn from(voting_procedures: VotingProcedures) -> Self {
        Self(
            voting_procedures
                .into_iter()
                .map(|(voter, votes)| {
                    (
                        voter,
                        votes
                            .into_iter()
                            .map(|(proposal, procedure)| (proposal.into(), procedure.vote))
                            .collect(),
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
pub mod test_vectors {
    use std::sync::LazyLock;

    use amaru_kernel::{
        Address, MemoizedDatum, MemoizedTransactionOutput, TransactionInput, include_json,
        serde_utils::hex_to_bytes,
    };
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct TestVector {
        pub meta: TestMeta,
        pub input: TestInput,
        pub expectations: TestExpectations,
    }

    #[derive(Debug, Deserialize)]
    pub struct TestMeta {
        pub title: String,
        pub description: String,
        pub plutus_version: u8,
    }

    #[derive(Deserialize)]
    pub struct TestInput {
        #[serde(rename = "transaction", deserialize_with = "hex_to_bytes")]
        pub transaction_bytes: Vec<u8>,
        #[serde(deserialize_with = "deserialize_utxo")]
        pub utxo: (TransactionInput, MemoizedTransactionOutput),
    }

    #[derive(Deserialize)]
    pub struct TestExpectations {
        #[serde(deserialize_with = "hex_to_bytes")]
        pub script_context: Vec<u8>,
    }

    fn deserialize_utxo<'de, D>(
        deserializer: D,
    ) -> Result<(TransactionInput, MemoizedTransactionOutput), D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct UtxoHelper {
            transaction: TransactionIdHelper,
            index: u32,
            address: String,
            value: ValueHelper,
            #[serde(default)]
            datum_hash: Option<String>,
            #[serde(default)]
            datum: Option<String>,
        }

        #[derive(Deserialize)]
        struct TransactionIdHelper {
            id: String,
        }

        #[derive(Deserialize)]
        struct ValueHelper {
            ada: AdaHelper,
        }

        #[derive(Deserialize)]
        struct AdaHelper {
            lovelace: u64,
        }

        let helpers: Vec<UtxoHelper> = Vec::deserialize(deserializer)?;
        let helper = helpers
            .first()
            .ok_or(serde::de::Error::custom("no provided utxo"))?;

        let tx_id_bytes = hex::decode(&helper.transaction.id).map_err(serde::de::Error::custom)?;

        let input = TransactionInput {
            transaction_id: tx_id_bytes.as_slice().into(),
            index: helper.index as u64,
        };

        let address_bytes = hex::decode(&helper.address).map_err(serde::de::Error::custom)?;
        let address = Address::from_bytes(&address_bytes).map_err(serde::de::Error::custom)?;
        let datum = match (&helper.datum_hash, &helper.datum) {
            (Some(hash_str), None) => {
                let hash_bytes = hex::decode(hash_str).map_err(serde::de::Error::custom)?;
                MemoizedDatum::Hash(hash_bytes.as_slice().into())
            }
            (None, Some(datum_str)) => MemoizedDatum::Inline(
                datum_str
                    .clone()
                    .try_into()
                    .map_err(serde::de::Error::custom)?,
            ),
            (None, None) => MemoizedDatum::None,
            (Some(_), Some(_)) => {
                return Err(serde::de::Error::custom(
                    "cannot have both datum_hash and datum",
                ));
            }
        };

        let output = MemoizedTransactionOutput {
            is_legacy: false,
            address,
            value: amaru_kernel::Value::Coin(helper.value.ada.lovelace),
            datum,
            script: None,
        };
        Ok((input, output))
    }

    static TEST_VECTORS: LazyLock<Vec<TestVector>> =
        LazyLock::new(|| include_json!("script-context-fixtures.json"));

    pub fn get_test_vectors(version: u8) -> Vec<&'static TestVector> {
        TEST_VECTORS
            .iter()
            .filter(|vector| vector.meta.plutus_version == version)
            .collect()
    }

    pub fn get_test_vector(title: &str) -> &'static TestVector {
        TEST_VECTORS
            .iter()
            .find(|vector| vector.meta.title == title)
            .unwrap_or_else(|| panic!("Test case not found: {title}"))
    }
}
