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
    cbor,
    script::{encode_script, serialize_memoized_script, PlaceholderScript},
    Address, AlonzoValue, AssetName, KeyValuePairs, LocalPseudoScript, Lovelace, MemoizedDatum,
    MemoizedNativeScript, MemoizedScript, MintedTransactionOutput, NonEmptyKeyValuePairs,
    ScriptHash, Value,
};

use minicbor_extra::{
    decode_chunk, heterogeneous_map, missing_field, unexpected_field, with_default_value,
};
use pallas_codec::minicbor::data::{IanaTag, Type};
use pallas_primitives::conway::PseudoScript;

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
    #[allow(unused_variables)] // ctx will be used in the future
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let data_type = d.datatype()?;

        if data_type == Type::MapIndef || data_type == Type::Map {
            decode_modern_output(d)
        } else if data_type == Type::ArrayIndef || data_type == Type::Array {
            decode_legacy_output(d)
        } else {
            Err(cbor::decode::Error::type_mismatch(data_type))
        }
    }
}

fn decode_legacy_output(
    d: &mut cbor::Decoder<'_>,
) -> Result<MemoizedTransactionOutput, cbor::decode::Error> {
    let len = d.array()?;

    Ok(MemoizedTransactionOutput {
        is_legacy: true,
        address: decode_address(d.bytes()?)?,
        value: d.decode()?,
        datum: match len {
            Some(2) => MemoizedDatum::None,
            Some(3) => d.decode()?,
            Some(_) => {
                return Err(cbor::decode::Error::message(format!(
                    "expected legacy transaction output array length of 2 or 3, got {len:?}",
                )))
            }
            None => {
                let datum = d.decode()?;
                decode_break(d)?;
                datum
            }
        },
        script: None,
    })
}

fn decode_modern_output(
    d: &mut cbor::Decoder<'_>,
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
                1 => state.1 = decode_chunk(d, |d| d.decode()),
                2 => state.2 = decode_chunk(d, |d| d.decode()),
                3 => state.3 = decode_chunk(d, decode_reference_script),
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

fn decode_reference_script(
    d: &mut cbor::Decoder<'_>,
) -> Result<Option<PseudoScript<MemoizedNativeScript>>, cbor::decode::Error> {
    if d.tag()? != IanaTag::Cbor.tag() {
        return Err(cbor::decode::Error::message("unexpected tag as script tag"));
    }
    let mut script_decoder: cbor::Decoder<'_> = cbor::Decoder::new(d.bytes()?);
    let failed_to_decode =
        |e| cbor::decode::Error::message(format!("failed to decode script: {e}"));

    Ok(Some(MemoizedScript::from(LocalPseudoScript(
        script_decoder.decode().map_err(failed_to_decode)?,
    ))))
}

fn decode_address(address_bytes: &[u8]) -> Result<Address, cbor::decode::Error> {
    Address::from_bytes(address_bytes)
        .map_err(|e| cbor::decode::Error::message(format!("invalid address: {e:?}")))
}

fn decode_break(d: &mut cbor::Decoder<'_>) -> Result<(), cbor::decode::Error> {
    if d.datatype()? != Type::Break {
        return Err(cbor::decode::Error::type_mismatch(Type::Break));
    }
    d.skip()
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
                script: output
                    .script_ref
                    .map(|script| MemoizedScript::from(LocalPseudoScript(script.unwrap()))),
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

#[allow(dead_code)] // not dead, used in a serde macro field.
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
#[allow(dead_code)] // not dead, used in a serde macro field.
fn serialize_value<S: serde::ser::Serializer>(
    value: &Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Value::Coin(coin) => serializer.serialize_u64(*coin),
        Value::Multiasset(coin, _) => serializer.serialize_u64(*coin),
    }
}

// FIXME: Eventually allow deserializing complete values, not just coins.
fn deserialize_value<'de, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<Value, D::Error> {
    Ok(Value::Coin(serde::Deserialize::deserialize(deserializer)?))
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
