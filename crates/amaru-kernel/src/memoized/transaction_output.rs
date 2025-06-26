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

use pallas_primitives::{conway::Multiasset, Bytes, Hash, PolicyId, PositiveCoin};

use crate::{
    Address, AlonzoValue, AssetName, KeyValuePairs, Lovelace, MemoizedDatum, MemoizedNativeScript,
    MemoizedPlutusData, MemoizedScript, MintedTransactionOutput, NonEmptyKeyValuePairs,
    PseudoScript, ScriptHash, Value, cbor,
    script::{PlaceholderScript, encode_script, serialize_memoized_script},
};

use pallas_primitives::{KeepRaw, PlutusData, conway::NativeScript};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct MemoizedTransactionOutput {
    #[serde(skip)]
    pub is_legacy: bool,

    #[serde(serialize_with = "serialize_address")]
    #[serde(deserialize_with = "deserialize_address")]
    pub address: Address,

    #[serde(serialize_with = "serialize_value")]
    #[serde(deserialize_with = "deserialize_value")]
    pub value: Value,

    pub datum: MemoizedDatum,

    #[serde(serialize_with = "serialize_script")]
    #[serde(deserialize_with = "deserialize_script")]
    pub script: Option<MemoizedScript>,
}

impl<'b, C> cbor::Decode<'b, C> for MemoizedTransactionOutput {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let data_type = d.datatype()?;
        let mut is_legacy = false;
        let mut address: Option<Address> = None;
        let mut value: Value = Value::Coin(0);
        let mut datum: MemoizedDatum = MemoizedDatum::None;
        let mut script: Option<MemoizedScript> = None;

        if data_type == cbor::data::Type::MapIndef || data_type == cbor::data::Type::Map {
            let map_size = d.map()?.unwrap_or(4);

            for _ in 0..map_size {
                // Check for break condition first (for indefinite maps)
                if d.datatype()? == cbor::data::Type::Break {
                    d.skip()?;
                    break;
                }

                let key = d.u8()?;
                match key {
                    0 => {
                        address = Some(Address::from_bytes(d.bytes()?).map_err(|e| {
                            cbor::decode::Error::message(format!("invalid address: {e:?}"))
                        })?);
                    }
                    1 => {
                        value = d.decode_with(ctx)?;
                    }
                    2 => {
                        d.array()?;
                        let datum_option = d.u8()?;
                        match datum_option {
                            0 => {
                                datum = MemoizedDatum::Hash(d.bytes()?.into());
                            }
                            1 => {
                                let tag = d.tag()?;

                                match tag.as_u64() {
                                    24 => {
                                        let plutus_data: KeepRaw<'_, PlutusData> =
                                            cbor::decode(d.bytes()?)?;
                                        let memoized_data = MemoizedPlutusData::from(plutus_data);
                                        datum = MemoizedDatum::Inline(memoized_data);
                                    }
                                    _ => {
                                        return Err(cbor::decode::Error::message(
                                            "unknown tag for datum tag",
                                        ));
                                    }
                                };
                            }
                            _ => {
                                return Err(cbor::decode::Error::message(format!(
                                    "unknown datum option: {}",
                                    datum_option
                                )));
                            }
                        }
                    }
                    3 => {
                        let tag = d.tag()?;

                        match tag.as_u64() {
                            24 => {
                                let mut script_decoder: cbor::Decoder<'_> =
                                    cbor::Decoder::new(d.bytes()?);
                                let pseudo_script: Result<
                                    PseudoScript<KeepRaw<'_, NativeScript>>,
                                    _,
                                > = script_decoder.decode_with(ctx);

                                script = match pseudo_script {
                                    Ok(pseudo_script_type) => match pseudo_script_type {
                                        PseudoScript::NativeScript(script_bytes) => {
                                            Some(PseudoScript::NativeScript(
                                                MemoizedNativeScript::from(script_bytes),
                                            ))
                                        }
                                        PseudoScript::PlutusV1Script(script_bytes) => {
                                            Some(PseudoScript::PlutusV1Script(script_bytes))
                                        }
                                        PseudoScript::PlutusV2Script(script_bytes) => {
                                            Some(PseudoScript::PlutusV2Script(script_bytes))
                                        }
                                        PseudoScript::PlutusV3Script(script_byte) => {
                                            Some(PseudoScript::PlutusV3Script(script_byte))
                                        }
                                    },
                                    Err(e) => {
                                        return Err(cbor::decode::Error::message(format!(
                                            "failed to decode script: {:?}",
                                            e
                                        )));
                                    }
                                };
                            }
                            _ => {
                                return Err(cbor::decode::Error::message(
                                    "unknown tag for script tag",
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(cbor::decode::Error::message("unknown key option"));
                    }
                }
            }
        } else if data_type == cbor::data::Type::ArrayIndef || data_type == cbor::data::Type::Array
        {
            is_legacy = true;
            d.array()?;
            address =
                Some(Address::from_bytes(d.bytes()?).map_err(|e| {
                    cbor::decode::Error::message(format!("invalid address: {e:?}"))
                })?);
            value = d.decode_with(ctx)?;

            if d.datatype()? == cbor::data::Type::Bytes {
                datum = MemoizedDatum::Hash(d.bytes()?.into());
            }
        } else {
            return Err(cbor::decode::Error::message(format!(
                "unexpected CBOR type for output: {:?}",
                data_type
            )));
        }

        let address = address
            .ok_or_else(|| cbor::decode::Error::message("missing required address field"))?;

        Ok(MemoizedTransactionOutput {
            is_legacy,
            address,
            value,
            datum,
            script,
        })
    }
}

impl<C> cbor::Encode<C> for MemoizedTransactionOutput {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        if self.is_legacy {
            e.begin_array()?;
            e.bytes(&self.address.to_vec())?;
            e.encode_with(&self.value, ctx)?;
            match self.datum {
                MemoizedDatum::None => (),
                MemoizedDatum::Hash(hash) => {
                    e.bytes(&hash[..])?;
                }
                MemoizedDatum::Inline(..) => unreachable!("legacy output with inline datum ?!"),
            }
            e.end()?;
        } else {
            e.begin_map()?;

            e.u8(0)?;
            e.bytes(&self.address.to_vec())?;

            e.u8(1)?;
            e.encode_with(&self.value, ctx)?;

            if !matches!(&self.datum, &MemoizedDatum::None) {
                e.u8(2)?;
            }
            e.encode_with(&self.datum, ctx)?;

            match &self.script {
                None => (),
                Some(script) => {
                    e.u8(3)?;
                    encode_script(script, e)?;
                }
            }

            e.end()?;
        }

        Ok(())
    }
}

impl<'a> TryFrom<MintedTransactionOutput<'a>> for MemoizedTransactionOutput {
    type Error = String;

    fn try_from(
        output: MintedTransactionOutput<'a>,
    ) -> Result<MemoizedTransactionOutput, Self::Error> {
        match output {
            MintedTransactionOutput::Legacy(output) => Ok(MemoizedTransactionOutput {
                is_legacy: true,
                address: Address::from_bytes(&output.address)
                    .map_err(|e| format!("invalid address: {e:?}"))?,
                value: from_legacy_value(output.amount)?,
                datum: MemoizedDatum::from(output.datum_hash),
                script: None,
            }),
            MintedTransactionOutput::PostAlonzo(output) => Ok(MemoizedTransactionOutput {
                is_legacy: false,
                address: Address::from_bytes(&output.address)
                    .map_err(|e| format!("invalid address: {e:?}"))?,
                value: output.value,
                datum: MemoizedDatum::from(output.datum_option),
                script: output.script_ref.map(|script| match script.unwrap() {
                    PseudoScript::NativeScript(native_script) => {
                        PseudoScript::NativeScript(MemoizedNativeScript::from(native_script))
                    }
                    PseudoScript::PlutusV1Script(plutus_script) => {
                        PseudoScript::PlutusV1Script(plutus_script)
                    }
                    PseudoScript::PlutusV2Script(plutus_script) => {
                        PseudoScript::PlutusV2Script(plutus_script)
                    }
                    PseudoScript::PlutusV3Script(plutus_script) => {
                        PseudoScript::PlutusV3Script(plutus_script)
                    }
                }),
            }),
        }
    }
}

// --------------------------------------------------------------------- Helpers

fn from_legacy_value(value: AlonzoValue) -> Result<Value, String> {
    let from_tokens = |tokens: KeyValuePairs<AssetName, Lovelace>| {
        NonEmptyKeyValuePairs::try_from(
            tokens
                .to_vec()
                .into_iter()
                .map(|(asset_name, quantity)| {
                    Ok((
                        asset_name,
                        quantity
                            .try_into()
                            .map_err(|e| format!("invalid quantity in legacy output: {e}"))?,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?,
        )
        .map_err(|_| "empty tokens under a policy?".to_string())
    };

    match value {
        AlonzoValue::Coin(coin) => Ok(Value::Coin(coin)),
        AlonzoValue::Multiasset(coin, assets) if assets.is_empty() => Ok(Value::Coin(coin)),
        AlonzoValue::Multiasset(coin, assets) => {
            let from_assets =
                |assets: KeyValuePairs<ScriptHash, KeyValuePairs<AssetName, Lovelace>>| {
                    NonEmptyKeyValuePairs::try_from(
                        assets
                            .to_vec()
                            .into_iter()
                            .map(|(policy_id, tokens)| Ok((policy_id, from_tokens(tokens)?)))
                            .collect::<Result<Vec<_>, String>>()?,
                    )
                    .map_err(|_| "assets cannot be empty due to pattern-guard".to_string())
                };

            Ok(Value::Multiasset(coin, from_assets(assets)?))
        }
    }
}

fn serialize_address<S: serde::ser::Serializer>(
    addr: &Address,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&addr.to_hex())
}

fn deserialize_address<'de, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<Address, D::Error> {
    let bytes: &str = serde::Deserialize::deserialize(deserializer)?;
    Address::from_hex(bytes).map_err(serde::de::Error::custom)
}

// FIXME: Eventually allow serializing complete values, not just coins.
fn serialize_value<S: serde::ser::Serializer>(
    value: &Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Value::Coin(coin) => serializer.serialize_u64(*coin),
        Value::Multiasset(coin, _) => serializer.serialize_u64(*coin),
    }
}

fn deserialize_value<'de, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<Value, D::Error> {
    #[derive(serde::Deserialize)]
    enum ValueHelper {
        Coin(u64),
        Multiasset(u64, Vec<(String, Vec<(String, u64)>)>),
    }

    let helper: ValueHelper = serde::Deserialize::deserialize(deserializer)?;

    match helper {
        ValueHelper::Coin(coin) => Ok(Value::Coin(coin)),
        ValueHelper::Multiasset(coin, multiasset_data) => {
            let mut converted_multiasset = Vec::new();

            for (policy_id, assets) in multiasset_data {
                let policy_id = hex::decode(&policy_id).map_err(|_| {
                    serde::de::Error::custom(format!("invalid hex string: {policy_id}"))
                })?;

                let mut converted_assets = Vec::new();
                for (asset_name, quantity) in assets {
                    let asset_name = hex::decode(&asset_name).map_err(|_| {
                        serde::de::Error::custom(format!("invalid hex string: {asset_name}"))
                    })?;

                    converted_assets.push((
                        Bytes::from(asset_name),
                        quantity.try_into().map_err(|_| {
                            serde::de::Error::custom(format!("invalid quantity value: {quantity}"))
                        })?,
                    ));
                }

                let policy_id: PolicyId = Hash::from(policy_id.as_slice());

                converted_multiasset.push((
                    policy_id,
                    NonEmptyKeyValuePairs::from_vec(converted_assets).ok_or(
                        serde::de::Error::custom(format!("empty asset bundle: {policy_id}")),
                    )?,
                ));
            }

            let multiasset: Multiasset<PositiveCoin> =
                Multiasset::from_vec(converted_multiasset)
                    .ok_or(serde::de::Error::custom("empty multiasset"))?;
            Ok(Value::Multiasset(coin, multiasset))
        }
    }
}

pub fn serialize_script<S: serde::ser::Serializer>(
    opt: &Option<MemoizedScript>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match opt {
        None => serializer.serialize_none(),
        Some(script) => serialize_memoized_script(script, serializer),
    }
}

pub fn deserialize_script<'de, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MemoizedScript>, D::Error> {
    match serde::Deserialize::deserialize(deserializer)? {
        None::<PlaceholderScript> => Ok(None),
        Some(placeholder) => Ok(Some(
            MemoizedScript::try_from(placeholder).map_err(serde::de::Error::custom)?,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor;
    use pallas_codec::minicbor::Encode;
    use pallas_crypto::hash::Hash;

    #[test]
    fn test_encode_decode_output_with_datum_hash() {
        let hash_bytes = [1u8; 32];
        let datum_hash = Hash::<32>::from(hash_bytes);

        let datum = MemoizedDatum::Hash(datum_hash);

        let original = MemoizedTransactionOutput {
            is_legacy: false,
            address: Address::from_hex(
                "61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335",
            )
            .unwrap(),
            value: Value::Coin(1500000),
            datum,
            script: None,
        };

        let mut encoder = cbor::Encoder::new(Vec::new());
        let mut ctx = ();
        original.encode(&mut encoder, &mut ctx).unwrap();
        let encoded_bytes = encoder.writer().clone();

        let mut decoder = cbor::Decoder::new(&encoded_bytes);
        let decoded: MemoizedTransactionOutput = decoder.decode_with(&mut ctx).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_encode_decode_output_with_datum_hash_legacy() {
        let hash_bytes = [1u8; 32];
        let datum_hash = Hash::<32>::from(hash_bytes);

        let datum = MemoizedDatum::Hash(datum_hash);

        let original = MemoizedTransactionOutput {
            is_legacy: true,
            address: Address::from_hex(
                "61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335",
            )
            .unwrap(),
            value: Value::Coin(1500000),
            datum,
            script: None,
        };

        let mut encoder = cbor::Encoder::new(Vec::new());
        let mut ctx = ();
        original.encode(&mut encoder, &mut ctx).unwrap();
        let encoded_bytes = encoder.writer().clone();

        let mut decoder = cbor::Decoder::new(&encoded_bytes);
        let decoded: MemoizedTransactionOutput = decoder.decode_with(&mut ctx).unwrap();

        assert_eq!(original, decoded);
    }
}
