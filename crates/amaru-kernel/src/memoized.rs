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
    cbor::bytes::ByteSlice, from_cbor, serde_utils::StringVisitor, AssetName, KeyValuePairs,
    Lovelace, PlutusScript, ScriptHash,
};
use pallas_addresses::Address;
use pallas_codec::{minicbor as cbor, minicbor::data::IanaTag, utils::Bytes};
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::{
    alonzo,
    conway::{
        KeepRaw, MintedDatumOption, MintedTransactionOutput, NativeScript, PseudoScript, Value,
    },
    DatumHash, NonEmptyKeyValuePairs, PlutusData,
};
use serde::ser::SerializeStruct;

// --------------------------------------------------- MemoizedTransactionOutput

#[derive(Debug, Clone, serde::Deserialize)]
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

    #[serde(serialize_with = "serialize_memoized_script")]
    #[serde(deserialize_with = "deserialize_memoized_script")]
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

fn from_legacy_value(value: alonzo::Value) -> Result<Value, String> {
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
        alonzo::Value::Coin(coin) => Ok(Value::Coin(coin)),
        alonzo::Value::Multiasset(coin, assets) if assets.is_empty() => Ok(Value::Coin(coin)),
        alonzo::Value::Multiasset(coin, assets) => {
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

// -------------------------------------------------------------- MemoizedScript

pub type MemoizedScript = PseudoScript<MemoizedNativeScript>;

#[allow(dead_code)] // not dead, used in a serde macro field.
fn serialize_memoized_script<S: serde::ser::Serializer>(
    script: &MemoizedScript,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut s = serializer.serialize_struct("MemoizedScript", 1)?;
    match script {
        // TODO: Adopt a less Rust-tainted encoding one day. Not doing it now because will remand
        // re-generating and re-encoding all the ledger test vectors which is only tangential to
        // the problem I am trying to solve.
        MemoizedScript::NativeScript(native) => {
            s.serialize_field("NativeScript", &hex::encode(native.original_bytes()))?;
        }
        MemoizedScript::PlutusV1Script(plutus) => {
            s.serialize_field("PlutusV1Script", &hex::encode(plutus.as_ref()))?;
        }
        MemoizedScript::PlutusV2Script(plutus) => {
            s.serialize_field("PlutusV1Script", &hex::encode(plutus.as_ref()))?;
        }
        MemoizedScript::PlutusV3Script(plutus) => {
            s.serialize_field("PlutusV1Script", &hex::encode(plutus.as_ref()))?;
        }
    }
    s.end()
}

fn deserialize_memoized_script<'de, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MemoizedScript>, D::Error> {
    #[derive(serde::Deserialize)]
    enum PlaceholderScript {
        NativeScript(Bytes),
        PlutusV1(Bytes),
        PlutusV2(Bytes),
        PlutusV3(Bytes),
    }

    Ok(match serde::Deserialize::deserialize(deserializer)? {
        Some(PlaceholderScript::NativeScript(bytes)) => Some(MemoizedScript::NativeScript(
            MemoizedNativeScript::try_from(bytes).map_err(serde::de::Error::custom)?,
        )),
        Some(PlaceholderScript::PlutusV1(bytes)) => {
            Some(MemoizedScript::PlutusV1Script(PlutusScript(bytes)))
        }
        Some(PlaceholderScript::PlutusV2(bytes)) => {
            Some(MemoizedScript::PlutusV2Script(PlutusScript(bytes)))
        }
        Some(PlaceholderScript::PlutusV3(bytes)) => {
            Some(MemoizedScript::PlutusV3Script(PlutusScript(bytes)))
        }
        None => None,
    })
}

pub fn script_original_bytes(script: &MemoizedScript) -> &[u8] {
    match script {
        MemoizedScript::NativeScript(native) => native.original_bytes(),
        MemoizedScript::PlutusV1Script(plutus) => plutus.as_ref(),
        MemoizedScript::PlutusV2Script(plutus) => plutus.as_ref(),
        MemoizedScript::PlutusV3Script(plutus) => plutus.as_ref(),
    }
}

pub fn encode_script<W: cbor::encode::Write>(
    script: &MemoizedScript,
    e: &mut cbor::Encoder<W>,
) -> Result<(), cbor::encode::Error<W::Error>> {
    e.tag(IanaTag::Cbor)?;

    let buffer = match script {
        MemoizedScript::NativeScript(native) => {
            let mut bytes = vec![
                130, // CBOR definite array of length 2
                0,   // Tag for Native Script
            ];
            bytes.extend_from_slice(native.original_bytes());
            bytes
        }
        MemoizedScript::PlutusV1Script(plutus) => {
            #[allow(clippy::unwrap_used)] // Infallible error.
            cbor::to_vec((1, Into::<&ByteSlice>::into(plutus.as_ref()))).unwrap()
        }
        MemoizedScript::PlutusV2Script(plutus) => {
            #[allow(clippy::unwrap_used)] // Infallible error.
            cbor::to_vec((2, Into::<&ByteSlice>::into(plutus.as_ref()))).unwrap()
        }
        MemoizedScript::PlutusV3Script(plutus) => {
            #[allow(clippy::unwrap_used)] // Infallible error.
            cbor::to_vec((3, Into::<&ByteSlice>::into(plutus.as_ref()))).unwrap()
        }
    };

    e.bytes(&buffer)?;

    Ok(())
}

// --------------------------------------------------------------- MemoizedDatum

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoizedDatum {
    None,
    Hash(DatumHash),
    Inline(MemoizedPlutusData),
}

impl<'de> serde::Deserialize<'de> for MemoizedDatum {
    fn deserialize<D: serde::de::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<MemoizedDatum, D::Error> {
        // TODO: rename those fields eventually to something less Rust-tainted.
        #[derive(serde::Deserialize)]
        enum PlaceholderDatum {
            Hash(DatumHash),
            Data(MemoizedPlutusData),
            #[serde(untagged)]
            Unit(()),
        }

        match serde::Deserialize::deserialize(deserializer)? {
            PlaceholderDatum::Unit(()) => Ok(MemoizedDatum::None),
            PlaceholderDatum::Hash(bytes) => Ok(MemoizedDatum::Hash(bytes)),
            PlaceholderDatum::Data(data) => Ok(MemoizedDatum::Inline(data)),
        }
    }
}

impl<C> cbor::Encode<C> for MemoizedDatum {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match &self {
            MemoizedDatum::None => (),
            MemoizedDatum::Hash(hash) => {
                e.array(2)?;
                e.u8(0)?;
                e.bytes(&hash[..])?;
            }
            MemoizedDatum::Inline(data) => {
                e.array(2)?;
                e.u8(1)?;
                e.tag(IanaTag::Cbor)?;
                e.bytes(data.original_bytes())?;
            }
        }

        Ok(())
    }
}

impl From<Option<MintedDatumOption<'_>>> for MemoizedDatum {
    fn from(opt: Option<MintedDatumOption<'_>>) -> Self {
        match opt {
            None => Self::None,
            Some(MintedDatumOption::Hash(hash)) => Self::Hash(hash),
            Some(MintedDatumOption::Data(cbor_wrap)) => {
                let data = cbor_wrap.unwrap();
                Self::Inline(MemoizedPlutusData {
                    original_bytes: Bytes::from(data.raw_cbor().to_vec()),
                    data: data.unwrap(),
                })
            }
        }
    }
}

impl From<Option<DatumHash>> for MemoizedDatum {
    fn from(opt: Option<DatumHash>) -> Self {
        opt.map(MemoizedDatum::Hash).unwrap_or(MemoizedDatum::None)
    }
}

// ---------------------------------------------------------- MemoizedPlutusData

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "&str")]
pub struct MemoizedPlutusData {
    original_bytes: Bytes,
    pub data: PlutusData,
}

impl MemoizedPlutusData {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn hash(&self) -> Hash<32> {
        Hasher::<256>::hash(&self.original_bytes)
    }
}
impl TryFrom<&str> for MemoizedPlutusData {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, data| Self {
            original_bytes,
            data,
        })
    }
}

impl TryFrom<String> for MemoizedPlutusData {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<Vec<u8>> for MemoizedPlutusData {
    type Error = cbor::decode::Error;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let data = cbor::decode(&bytes)?;
        Ok(Self {
            original_bytes: Bytes::from(bytes),
            data,
        })
    }
}

// -------------------------------------------------------- MemoizedNativeScript

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "&str")]
pub struct MemoizedNativeScript {
    original_bytes: Bytes,
    pub expr: NativeScript,
}

impl MemoizedNativeScript {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl TryFrom<Bytes> for MemoizedNativeScript {
    type Error = String;

    fn try_from(original_bytes: Bytes) -> Result<Self, Self::Error> {
        let expr = from_cbor(&original_bytes)
            .ok_or_else(|| "invalid serialized native script".to_string())?;

        Ok(Self {
            original_bytes,
            expr,
        })
    }
}

impl From<KeepRaw<'_, NativeScript>> for MemoizedNativeScript {
    fn from(script: KeepRaw<'_, NativeScript>) -> Self {
        Self {
            original_bytes: Bytes::from(script.raw_cbor().to_vec()),
            expr: script.unwrap(),
        }
    }
}

impl TryFrom<&str> for MemoizedNativeScript {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, expr| Self {
            original_bytes,
            expr,
        })
    }
}

impl TryFrom<String> for MemoizedNativeScript {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

// --------------------------------------------------------------------- Helpers

fn blanket_try_from_hex_bytes<T, I: for<'d> cbor::Decode<'d, ()>>(
    s: &str,
    new: impl Fn(Bytes, I) -> T,
) -> Result<T, String> {
    let original_bytes = Bytes::from(hex::decode(s.as_bytes()).map_err(|e| e.to_string())?);

    let value =
        from_cbor(&original_bytes).ok_or_else(|| "failed to decode from CBOR".to_string())?;

    Ok(new(original_bytes, value))
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
    let bytes = deserializer.deserialize_str(StringVisitor)?;
    Address::from_hex(&bytes).map_err(serde::de::Error::custom)
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
