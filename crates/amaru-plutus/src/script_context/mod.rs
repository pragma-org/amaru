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
    Certificate, KeyValuePairs, MemoizedTransactionOutput, PlutusData, Redeemer, TransactionInput,
    Voter,
};
use std::{collections::BTreeMap, ops::Deref};

pub mod v1;
pub mod v2;
pub mod v3;

pub mod tx_info;
pub use tx_info::*;

pub trait IsPrePlutusVersion3 {}
impl IsPrePlutusVersion3 for PlutusVersion<1> {}
impl IsPrePlutusVersion3 for PlutusVersion<2> {}

use crate::{IsKnownPlutusVersion, PlutusDataError, PlutusVersion, ToPlutusData};

/// One of the arguments passed to a Plutus validator.
///
///
/// It contains information about the transaction which is being validated, and the specific script which is being run.
///
/// A `ScriptContext` can only be constructed via the [`ScriptContext::new`](Self::new) function.
///
/// The serialized representation of `ScriptContext` may be different for each `PlutusVersion`,
/// so it is important to specify the correct `PlutusVersion` when serializing.
pub struct ScriptContext<'a> {
    tx_info: &'a TxInfo,
    redeemer: &'a Redeemer,
    datum: Option<&'a PlutusData>,
    script_purpose: &'a ScriptPurpose,
}

impl<'a> ScriptContext<'a> {
    /// Construct a new [`ScriptContext`] for a specific script execution (specified by the `Redeemer`).
    ///
    /// Returns `None` if the provided `Redeemer` does not exist in the `TxInfo`
    pub fn new(tx_info: &'a TxInfo, redeemer: &'a Redeemer) -> Option<Self> {
        let purpose = tx_info
            .redeemers
            .0
            .iter()
            .find_map(|(purpose, tx_redeemer)| {
                if redeemer.tag == tx_redeemer.tag && redeemer.index == tx_redeemer.index {
                    Some(purpose)
                } else {
                    None
                }
            });

        let datum = if amaru_kernel::ScriptPurpose::Spend == redeemer.tag {
            tx_info
                .inputs
                .get(redeemer.index as usize)
                .and_then(|output_ref| match &output_ref.output.datum {
                    DatumOption::None => None,
                    DatumOption::Hash(hash) => tx_info.data.0.get(hash),
                    DatumOption::Inline(plutus_data) => Some(plutus_data),
                })
        } else {
            None
        };

        purpose.map(|script_purpose| ScriptContext {
            tx_info,
            redeemer,
            datum,
            script_purpose,
        })
    }

    /// Serialize `ScriptContext` to a list of arguments to be passed to a Plutus validator.
    ///
    /// For both PlutusV1 and PlutusV2, the list consists of:
    /// `[datum?, redeemer, script_context]`
    ///
    /// For PlutusV3 the lists consists of:
    /// `[script_context]`
    pub fn to_script_args<const V: u8>(
        &self,
        _version: PlutusVersion<V>,
    ) -> Result<Vec<PlutusData>, PlutusDataError>
    where
        PlutusVersion<V>: IsKnownPlutusVersion,
    {
        match V {
            1 => self.v1_script_args(),
            2 => self.v2_script_args(),
            3 => self.v3_script_args(),
            _ => unreachable!("unknown PlutusVersion passed to to_script_args"),
        }
    }

    fn v1_script_args(&self) -> Result<Vec<PlutusData>, PlutusDataError> {
        let mut args = vec![];
        if let Some(datum) = self.datum {
            args.push(datum.clone());
        }

        args.push(self.redeemer.data.clone());
        args.push(<Self as ToPlutusData<1>>::to_plutus_data(self)?);

        Ok(args)
    }

    fn v2_script_args(&self) -> Result<Vec<PlutusData>, PlutusDataError> {
        let mut args = vec![];
        if let Some(datum) = self.datum {
            args.push(datum.clone());
        }

        args.push(self.redeemer.data.clone());
        args.push(<Self as ToPlutusData<2>>::to_plutus_data(self)?);

        Ok(args)
    }

    fn v3_script_args(&self) -> Result<Vec<PlutusData>, PlutusDataError> {
        Ok(vec![<Self as ToPlutusData<3>>::to_plutus_data(self)?])
    }
}

/// A subset of the UTxO set.
///
/// Maps from a `TransactionInput` to a `MemoizedTransactionOutput`
pub struct Utxos(BTreeMap<TransactionInput, MemoizedTransactionOutput>);

impl Utxos {
    /// Resolve an input to the output it references, returning an [`OutputRef`]
    ///
    ///
    /// Returns `None` when the input cannot be found in the UTxO slice.
    pub fn resolve_input(&self, input: &TransactionInput) -> Option<OutputRef> {
        self.0.get(input).map(|utxo| OutputRef {
            input: input.clone(),
            output: utxo.clone().into(),
        })
    }
}

impl Deref for Utxos {
    type Target = BTreeMap<TransactionInput, MemoizedTransactionOutput>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<BTreeMap<TransactionInput, MemoizedTransactionOutput>> for Utxos {
    fn from(value: BTreeMap<TransactionInput, MemoizedTransactionOutput>) -> Self {
        Self(value)
    }
}

#[cfg(test)]
pub mod test_vectors {
    use std::{collections::BTreeMap, sync::LazyLock};

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
        pub utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    }

    #[derive(Deserialize)]
    pub struct TestExpectations {
        pub script_context: String,
    }

    pub struct MemoizedTransactionOutputWrapper(pub MemoizedTransactionOutput);

    fn deserialize_utxo<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<TransactionInput, MemoizedTransactionOutput>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UtxoVisitor;

        impl<'a> serde::de::Visitor<'a> for UtxoVisitor {
            type Value = BTreeMap<TransactionInput, MemoizedTransactionOutput>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("UTxOs")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::SeqAccess<'a>,
            {
                let mut utxo_map = BTreeMap::new();

                while let Some(entry) = seq.next_element::<UtxoEntryHelper>()? {
                    let tx_id_bytes =
                        hex::decode(&entry.transaction.id).map_err(serde::de::Error::custom)?;

                    let input = TransactionInput {
                        transaction_id: tx_id_bytes.as_slice().into(),
                        index: entry.index,
                    };

                    utxo_map.insert(input, entry.output.0);
                }

                Ok(utxo_map)
            }
        }

        #[derive(Deserialize)]
        struct UtxoEntryHelper {
            transaction: TransactionIdHelper,
            index: u64,
            #[serde(flatten)]
            output: MemoizedTransactionOutputWrapper,
        }

        #[derive(Deserialize)]
        struct TransactionIdHelper {
            id: String,
        }

        deserializer.deserialize_seq(UtxoVisitor)
    }

    impl<'a> serde::Deserialize<'a> for MemoizedTransactionOutputWrapper {
        fn deserialize<D: serde::Deserializer<'a>>(deserializer: D) -> Result<Self, D::Error> {
            #[derive(serde::Deserialize)]
            #[serde(field_identifier, rename_all = "snake_case")]
            enum Field {
                Address,
                Value,
                DatumHash,
                Datum,
                Script,
            }

            const FIELDS: &[&str] = &["address", "value", "datum", "datum_hash", "script"];

            struct TransactionOutputVisitor;

            impl<'a> serde::de::Visitor<'a> for TransactionOutputVisitor {
                type Value = MemoizedTransactionOutputWrapper;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("TransationOutput")
                }

                fn visit_map<V>(
                    self,
                    mut map: V,
                ) -> Result<MemoizedTransactionOutputWrapper, V::Error>
                where
                    V: serde::de::MapAccess<'a>,
                {
                    let mut address = None;
                    let mut value = None;
                    let mut datum = MemoizedDatum::None;
                    let script = None;

                    let assert_only_datum_or_hash = |datum: &MemoizedDatum| {
                        if datum != &MemoizedDatum::None {
                            return Err("cannot have both datum_hash and datum".to_string());
                        }

                        Ok(())
                    };

                    while let Some(key) = map.next_key()? {
                        match key {
                            Field::Address => {
                                let string: String = map.next_value()?;
                                let bytes =
                                    hex::decode(string).map_err(serde::de::Error::custom)?;
                                address = Some(
                                    Address::from_bytes(&bytes)
                                        .map_err(serde::de::Error::custom)?,
                                );
                            }
                            Field::Value => {
                                let helper: BTreeMap<String, BTreeMap<String, u64>> =
                                    map.next_value()?;
                                value = Some(amaru_kernel::Value::Coin(
                                    *helper
                                        .get("ada")
                                        .ok_or_else(|| serde::de::Error::missing_field("ada"))?
                                        .get("lovelace")
                                        .ok_or_else(|| {
                                            serde::de::Error::missing_field("lovelace")
                                        })?,
                                ));
                            }
                            Field::Datum => {
                                assert_only_datum_or_hash(&datum)
                                    .map_err(serde::de::Error::custom)?;
                                let string: String = map.next_value()?;
                                datum = MemoizedDatum::Inline(
                                    string.try_into().map_err(serde::de::Error::custom)?,
                                );
                            }
                            Field::DatumHash => {
                                assert_only_datum_or_hash(&datum)
                                    .map_err(serde::de::Error::custom)?;
                                let string: String = map.next_value()?;
                                let bytes: Vec<u8> =
                                    hex::decode(string).map_err(serde::de::Error::custom)?;
                                datum = MemoizedDatum::Hash(bytes.as_slice().into())
                            }
                            Field::Script => {
                                unimplemented!("script in UTxO not yet supported");
                            }
                        }
                    }

                    Ok(MemoizedTransactionOutputWrapper(
                        MemoizedTransactionOutput {
                            is_legacy: false,
                            address: address
                                .ok_or_else(|| serde::de::Error::missing_field("address"))?,
                            value: value.ok_or_else(|| serde::de::Error::missing_field("value"))?,
                            datum,
                            script,
                        },
                    ))
                }
            }

            deserializer.deserialize_struct("TransationOutput", FIELDS, TransactionOutputVisitor)
        }
    }

    static TEST_VECTORS: LazyLock<Vec<TestVector>> =
        LazyLock::new(|| include_json!("script-context-fixtures.json"));

    pub fn get_test_vectors(version: u8) -> Vec<&'static TestVector> {
        TEST_VECTORS
            .iter()
            .filter(|vector| vector.meta.plutus_version == version)
            .collect()
    }

    pub fn get_test_vector(title: &str, ver: u8) -> &'static TestVector {
        get_test_vectors(ver)
            .iter()
            .find(|vector| vector.meta.title == title)
            .unwrap_or_else(|| panic!("Test case not found: {title}"))
    }
}

#[cfg(test)]
mod tests {

    use amaru_kernel::{Bytes, Network, ShelleyAddress, ShelleyDelegationPart, StakePayload};
    use proptest::{
        prelude::{Just, Strategy, any, prop},
        prop_assert, prop_oneof, proptest,
    };

    use crate::ToPlutusData;

    use super::*;

    fn network_strategy() -> impl Strategy<Value = Network> {
        prop_oneof![
            Just(Network::Testnet),
            Just(Network::Mainnet),
            any::<u8>().prop_map(Network::from),
        ]
    }

    fn stake_address_strategy() -> impl Strategy<Value = StakeAddress> {
        (prop::bool::ANY, any::<[u8; 28]>(), network_strategy()).prop_map(
            |(is_script, hash_bytes, network)| {
                let delegation: ShelleyDelegationPart = if is_script {
                    ShelleyDelegationPart::Script(hash_bytes.into())
                } else {
                    ShelleyDelegationPart::Key(hash_bytes.into())
                };

                // It is not possible to construct a StakeAddress from the parts of it in Pallas.
                // Instead, we're constructing a ShelelyAddress and then converting it to a StakeAddress.
                // The conversion uses the ShelleyDelegationPart, so the ShelleyPaymentPart is totally arbitrary here
                let shelley_addr = ShelleyAddress::new(
                    network,
                    amaru_kernel::ShelleyPaymentPart::Key(hash_bytes.into()),
                    delegation,
                );

                StakeAddress(shelley_addr.try_into().unwrap())
            },
        )
    }

    #[test]
    fn proptest_stake_address_ordering() {
        proptest!(|(addresses in prop::collection::vec(stake_address_strategy(), 20..100))| {
            let mut sorted = addresses.clone();
            sorted.sort();


            for window in sorted.windows(2) {
                let a = &window[0];
                let b = &window[1];

                fn network_tag(network: Network) -> u8 {
                    match network {
                        Network::Testnet => 0,
                        Network::Mainnet => 1,
                        Network::Other(tag) => tag,
                    }
                }

                let net_a = a.0.network();
                let net_b = b.0.network();


                // We sort by network first (testnet, mainnet, other by tag)
                if net_a != net_b {
                    prop_assert!(
                        network_tag(net_a) < network_tag(net_b),
                        "Network ordering violated: {:?} should be < {:?}",
                        network_tag(net_a),
                        network_tag(net_b)
                    );
                } else {
                    match (a.0.payload(), b.0.payload()) {
                        // Script < Stake
                        (StakePayload::Script(_), StakePayload::Stake(_)) => {
                            // This is correct
                        }
                        (StakePayload::Stake(_), StakePayload::Script(_)) => {
                            prop_assert!(false, "Payload type ordering violated: Stake should not come before Script");
                        }
                        // Same payload compare bytes
                        (StakePayload::Script(h1), StakePayload::Script(h2)) => {
                            prop_assert!(
                                h1 <= h2,
                                "Script hash ordering violated: {:?} should be <= {:?}",
                                h1, h2
                            );
                        }
                        (StakePayload::Stake(h1), StakePayload::Stake(h2)) => {
                            prop_assert!(
                                h1 <= h2,
                                "Stake hash ordering violated: {:?} should be <= {:?}",
                                h1, h2
                            );
                        }
                    }
                }
            }
        });
    }

    #[test]
    fn proptest_value_zero_ada_excluded_in_v3() {
        proptest!(|(policies in prop::collection::vec(any::<[u8; 28]>(), 1..5))| {
            let mut value_map = BTreeMap::new();

            // We should be excluding ADA values with a quantity of zero in Plutus V3
            let mut ada_map = BTreeMap::new();
            ada_map.insert(Bytes::from(vec![]), 0u64);
            value_map.insert(CurrencySymbol::Lovelace, ada_map);

            for policy_bytes in policies {
                let mut asset_map = BTreeMap::new();
                asset_map.insert(Bytes::from(vec![1]), 100);
                value_map.insert(CurrencySymbol::Native(policy_bytes.into()), asset_map);
            }

            let value = Value(value_map);
            let plutus_data = <Value as ToPlutusData<3>>::to_plutus_data(&value)?;

            #[allow(clippy::wildcard_enum_match_arm)]
            match plutus_data {
                PlutusData::Map(KeyValuePairs::Def(pairs)) => {
                    let has_ada = pairs.iter().any(|(key, _)| {
                        matches!(key, PlutusData::BoundedBytes(b) if b.is_empty())
                    });

                    prop_assert!(!has_ada,
                        "V3 Value should exclude ADA entry when amount is zero. Found {} pairs",
                        pairs.len());

                    prop_assert!(!pairs.is_empty(),
                        "Should still have non-ADA assets in the map");
                }
                other => {
                    prop_assert!(false, "Value should encode as Map, got: {:?}", other);
                }
            }
        });
    }

    #[test]
    fn proptest_value_nonzero_ada_included_in_v3() {
        proptest!(|(
            ada_amount in 1u64..,
            policies in prop::collection::vec(any::<[u8; 28]>(), 1..5)
        )| {
            let mut value_map = BTreeMap::new();

            let mut ada_map = BTreeMap::new();
            ada_map.insert(Bytes::from(vec![]), ada_amount);
            value_map.insert(CurrencySymbol::Lovelace, ada_map);

            for policy_bytes in policies {
                let mut asset_map = BTreeMap::new();
                asset_map.insert(Bytes::from(vec![1]), 100);
                value_map.insert(CurrencySymbol::Native(policy_bytes.into()), asset_map);
            }

            let value = Value(value_map);
            let plutus_data = <Value as ToPlutusData<3>>::to_plutus_data(&value)?;

            #[allow(clippy::wildcard_enum_match_arm)]
            match plutus_data {
                PlutusData::Map(KeyValuePairs::Def(pairs)) => {
                    let ada_entry = pairs.iter().find(|(key, _)| {
                        matches!(key, PlutusData::BoundedBytes(b) if b.is_empty())
                    });

                    prop_assert!(ada_entry.is_some(),
                        "V3 Value should include ADA entry when amount is non-zero");
                }
                other => {
                    prop_assert!(false, "Value should encode as Map, got: {:?}", other);
                }
            }
        });
    }
}
