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
    Address, AlonzoValue, AssetName, KeyValuePairs, Lovelace, MemoizedDatum, MemoizedNativeScript,
    MemoizedScript, MintedTransactionOutput, NonEmptyKeyValuePairs, PseudoScript, ScriptHash,
    Value,
};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemoizedTransactionOutput {
    #[serde(skip)]
    is_legacy: bool,

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
    // TODO: This implementation still copies the bytes when we convert from the minted version to
    // the owned version. To avoid that, we should actually rewrite the decoders for all the
    // memoized structures, so that they can retain their original bytes.
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let minted: MintedTransactionOutput<'_> = d.decode_with(ctx)?;
        Self::try_from(minted).map_err(cbor::decode::Error::message)
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
