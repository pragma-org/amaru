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
    LegacyKeyValuePairs, OutputReference, PlutusData, PlutusDatums, PlutusMint, PlutusRedeemers, PlutusStakeAddress,
    PlutusVotes, PlutusWithdrawals, ScriptContext, ScriptInfo, ScriptPurpose, TxInfo,
};

pub mod v1;
pub mod v2;
pub mod v3;

pub trait IsPrePlutusVersion3 {}
impl IsPrePlutusVersion3 for PlutusVersion<1> {}
impl IsPrePlutusVersion3 for PlutusVersion<2> {}

use crate::{IsKnownPlutusVersion, PlutusDataError, PlutusVersion, ToPlutusData};

/// Extension trait providing serialization of a [`ScriptContext`] to the argument
/// list passed to a Plutus validator.
pub trait ToScriptArgs {
    /// Serialize `ScriptContext` to a list of arguments to be passed to a Plutus validator.
    ///
    /// For both PlutusV1 and PlutusV2, the list consists of:
    /// `[datum?, redeemer, script_context]`
    ///
    /// For PlutusV3 the lists consists of:
    /// `[script_context]`
    fn to_script_args<const V: u8>(&self, _version: PlutusVersion<V>) -> Result<Vec<PlutusData>, PlutusDataError>
    where
        PlutusVersion<V>: IsKnownPlutusVersion;
}

impl<'a> ToScriptArgs for ScriptContext<'a> {
    fn to_script_args<const V: u8>(&self, _version: PlutusVersion<V>) -> Result<Vec<PlutusData>, PlutusDataError>
    where
        PlutusVersion<V>: IsKnownPlutusVersion,
    {
        match V {
            1 => v1_script_args(self),
            2 => v2_script_args(self),
            3 => v3_script_args(self),
            _ => unreachable!("unknown PlutusVersion passed to to_script_args"),
        }
    }
}

fn v1_script_args(ctx: &ScriptContext<'_>) -> Result<Vec<PlutusData>, PlutusDataError> {
    let mut args = vec![];
    if let Some(datum) = ctx.datum {
        args.push(datum.clone());
    }

    args.push(ctx.redeemer_data.clone());
    args.push(<ScriptContext<'_> as ToPlutusData<1>>::to_plutus_data(ctx)?);

    Ok(args)
}

fn v2_script_args(ctx: &ScriptContext<'_>) -> Result<Vec<PlutusData>, PlutusDataError> {
    let mut args = vec![];
    if let Some(datum) = ctx.datum {
        args.push(datum.clone());
    }
    args.push(ctx.redeemer_data.clone());
    args.push(<ScriptContext<'_> as ToPlutusData<2>>::to_plutus_data(ctx)?);

    Ok(args)
}

fn v3_script_args(ctx: &ScriptContext<'_>) -> Result<Vec<PlutusData>, PlutusDataError> {
    Ok(vec![<ScriptContext<'_> as ToPlutusData<3>>::to_plutus_data(ctx)?])
}

impl<'a, const V: u8> ToPlutusData<V> for PlutusRedeemers<'a>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    ScriptPurpose<'a>: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let converted: Result<Vec<_>, _> = self
            .values()
            .map(|entry| {
                Ok((<ScriptPurpose<'_> as ToPlutusData<V>>::to_plutus_data(&entry.purpose)?, entry.data.clone()))
            })
            .collect();

        Ok(PlutusData::Map(LegacyKeyValuePairs::Def(converted?)))
    }
}

#[cfg(test)]
pub mod test_vectors {
    use std::{collections::BTreeMap, sync::LazyLock};

    use amaru_kernel::{
        Address, MemoizedDatum, MemoizedTransactionOutput, MemoizedValue, TransactionInput, include_json,
        utils::serde::hex_to_bytes,
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
                    let tx_id_bytes = hex::decode(&entry.transaction.id).map_err(serde::de::Error::custom)?;

                    let input = TransactionInput { transaction_id: tx_id_bytes.as_slice().into(), index: entry.index };

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

                fn visit_map<V>(self, mut map: V) -> Result<MemoizedTransactionOutputWrapper, V::Error>
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
                                let bytes = hex::decode(string).map_err(serde::de::Error::custom)?;
                                address = Some(Address::from_bytes(&bytes).map_err(serde::de::Error::custom)?);
                            }
                            Field::Value => {
                                let helper: BTreeMap<String, BTreeMap<String, u64>> = map.next_value()?;
                                value = Some(amaru_kernel::Value::Coin(
                                    *helper
                                        .get("ada")
                                        .ok_or_else(|| serde::de::Error::missing_field("ada"))?
                                        .get("lovelace")
                                        .ok_or_else(|| serde::de::Error::missing_field("lovelace"))?,
                                ));
                            }
                            Field::Datum => {
                                assert_only_datum_or_hash(&datum).map_err(serde::de::Error::custom)?;
                                let string: String = map.next_value()?;
                                datum = MemoizedDatum::Inline(string.try_into().map_err(serde::de::Error::custom)?);
                            }
                            Field::DatumHash => {
                                assert_only_datum_or_hash(&datum).map_err(serde::de::Error::custom)?;
                                let string: String = map.next_value()?;
                                let bytes: Vec<u8> = hex::decode(string).map_err(serde::de::Error::custom)?;
                                datum = MemoizedDatum::Hash(bytes.as_slice().into())
                            }
                            Field::Script => {
                                unimplemented!("script in UTxO not yet supported");
                            }
                        }
                    }

                    let value = value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                    let value = MemoizedValue::new(value).map_err(serde::de::Error::custom)?;

                    Ok(MemoizedTransactionOutputWrapper(MemoizedTransactionOutput::new(
                        false,
                        address.ok_or_else(|| serde::de::Error::missing_field("address"))?,
                        value,
                        datum,
                        script,
                    )))
                }
            }

            deserializer.deserialize_struct("TransationOutput", FIELDS, TransactionOutputVisitor)
        }
    }

    static TEST_VECTORS: LazyLock<Vec<TestVector>> = LazyLock::new(|| include_json!("script-context-fixtures.json"));

    pub fn get_test_vectors(version: u8) -> Vec<&'static TestVector> {
        TEST_VECTORS.iter().filter(|vector| vector.meta.plutus_version == version).collect()
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
    use amaru_kernel::{Bytes, Hash, NonEmptyKeyValuePairs, PositiveCoin};
    use proptest::{
        prelude::{any, prop},
        prop_assert, proptest,
    };

    use super::*;
    use crate::ToPlutusData;

    /// Build a multiasset [`Value`](amaru_kernel::Value) carrying the given lovelace `coin` and one
    /// asset (quantity 100) under each of `policies`.
    fn multiasset_value(coin: u64, policies: &[[u8; 28]]) -> amaru_kernel::Value {
        let multiasset = NonEmptyKeyValuePairs::try_from(
            policies
                .iter()
                .map(|policy| {
                    let assets = NonEmptyKeyValuePairs::try_from(vec![(
                        Bytes::from(vec![1u8]),
                        PositiveCoin::try_from(100u64).unwrap(),
                    )])
                    .unwrap();
                    (Hash::from(*policy), assets.as_pallas())
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();

        amaru_kernel::Value::Multiasset(coin, multiasset.as_pallas())
    }

    #[test]
    fn proptest_value_zero_ada_excluded_in_v3() {
        // We should be excluding ADA values with a quantity of zero in Plutus V3
        proptest!(|(policies in prop::collection::vec(any::<[u8; 28]>(), 1..5))| {
            let value = multiasset_value(0, &policies);
            let plutus_data = <amaru_kernel::Value as ToPlutusData<3>>::to_plutus_data(&value)?;

            #[allow(clippy::wildcard_enum_match_arm)]
            match plutus_data {
                PlutusData::Map(pairs) => {
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
            let value = multiasset_value(ada_amount, &policies);
            let plutus_data = <amaru_kernel::Value as ToPlutusData<3>>::to_plutus_data(&value)?;

            #[allow(clippy::wildcard_enum_match_arm)]
            match plutus_data {
                PlutusData::Map(pairs) => {
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
