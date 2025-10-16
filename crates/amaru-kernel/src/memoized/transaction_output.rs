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

use crate::{
    Address, AlonzoValue, AssetName, KeyValuePairs, Legacy, Lovelace, MemoizedDatum,
    MemoizedScript, MintedTransactionOutput, NonEmptyKeyValuePairs, ScriptHash, Value, cbor,
    decode_script, from_minted_script,
    script::{PlaceholderScript, encode_script, serialize_memoized_script},
};
use amaru_minicbor_extra::{
    decode_break, decode_chunk, heterogeneous_map, missing_field, unexpected_field,
    with_default_value,
};
use pallas_codec::minicbor::data::Type;
use pallas_primitives::{Bytes, Hash, PolicyId, PositiveCoin, conway::Multiasset};

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

        if matches!(data_type, Type::MapIndef | Type::Map) {
            decode_modern_output(d, ctx)
        } else if matches!(data_type, Type::ArrayIndef | Type::Array) {
            decode_legacy_output(d, ctx)
        } else {
            Err(cbor::decode::Error::type_mismatch(data_type))
        }
    }
}

fn decode_legacy_output<C>(
    d: &mut cbor::Decoder<'_>,
    ctx: &mut C,
) -> Result<MemoizedTransactionOutput, cbor::decode::Error> {
    let len = d.array()?;

    Ok(MemoizedTransactionOutput {
        is_legacy: true,
        address: decode_address(d.bytes()?)?,
        value: d.decode_with(ctx)?,
        datum: match len {
            Some(2) => MemoizedDatum::None,
            Some(3) => d.decode_with::<_, Legacy<_>>(ctx)?.0,
            Some(_) => {
                return Err(cbor::decode::Error::message(format!(
                    "expected legacy transaction output array length of 2 or 3, got {len:?}",
                )));
            }
            None => {
                if decode_break(d, len)? {
                    MemoizedDatum::None
                } else {
                    let datum: Legacy<MemoizedDatum> = d.decode_with(ctx)?;
                    if !decode_break(d, len)? {
                        return Err(cbor::decode::Error::message(
                            "expected break after legacy transaction output datum",
                        ));
                    }
                    datum.0
                }
            }
        },
        script: None,
    })
}

fn decode_modern_output<C>(
    d: &mut cbor::Decoder<'_>,
    ctx: &mut C,
) -> Result<MemoizedTransactionOutput, cbor::decode::Error> {
    let (address, value, datum, script) = heterogeneous_map(
        d,
        (
            missing_field::<MemoizedTransactionOutput, _>(0),
            missing_field::<MemoizedTransactionOutput, _>(1),
            with_default_value(MemoizedDatum::None),
            with_default_value(None),
        ),
        |d| d.u8(),
        |d, state, field| {
            match field {
                0 => state.0 = decode_chunk(d, |d| decode_address(d.bytes()?)),
                1 => state.1 = decode_chunk(d, |d| d.decode_with(ctx)),
                2 => state.2 = decode_chunk(d, |d| d.decode_with(ctx)),
                3 => state.3 = decode_chunk(d, |d| Ok(Some(decode_script(d, ctx)?))),
                _ => return unexpected_field::<MemoizedTransactionOutput, _>(field),
            }
            Ok(())
        },
    )?;

    Ok(MemoizedTransactionOutput {
        is_legacy: false,
        address: address()?,
        value: value()?,
        datum: datum()?,
        script: script()?,
    })
}

fn decode_address(address_bytes: &[u8]) -> Result<Address, cbor::decode::Error> {
    Address::from_bytes(address_bytes)
        .map_err(|e| cbor::decode::Error::message(format!("invalid address: {e:?}")))
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
                script: output.script_ref.map(from_minted_script),
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

    #[test]
    fn test_encode_decode_output_no_datum_no_script() {
        let original = MemoizedTransactionOutput {
            is_legacy: false,
            address: Address::from_hex(
                "61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335",
            )
            .unwrap(),
            value: Value::Coin(1500000),
            datum: MemoizedDatum::None,
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
