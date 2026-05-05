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

use pallas_primitives::conway::Multiasset;

use crate::{
    Address, Bytes, Hash, Legacy, MemoizedDatum, MemoizedScript, MemoizedValue, NonEmptyKeyValuePairs, PositiveCoin,
    ShelleyDelegationPart, StakeCredential, Value, cbor, decode_script, encode_script, serialize_memoized_script,
    size::CREDENTIAL, to_cbor,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "MemoizedTransactionOutputDe")]
pub struct MemoizedTransactionOutput {
    #[serde(skip)]
    original_size: usize,

    #[serde(skip)]
    pub is_legacy: bool,

    #[serde(serialize_with = "serialize_address")]
    pub address: Address,

    #[serde(serialize_with = "serialize_value")]
    pub value: MemoizedValue,

    pub datum: MemoizedDatum,

    #[serde(serialize_with = "serialize_script")]
    pub script: Option<MemoizedScript>,
}

#[derive(serde::Deserialize)]
struct MemoizedTransactionOutputDe {
    #[serde(deserialize_with = "deserialize_address")]
    address: Address,

    #[serde(deserialize_with = "deserialize_value")]
    value: MemoizedValue,

    datum: MemoizedDatum,

    #[serde(deserialize_with = "deserialize_script")]
    script: Option<MemoizedScript>,
}

impl From<MemoizedTransactionOutputDe> for MemoizedTransactionOutput {
    fn from(de: MemoizedTransactionOutputDe) -> Self {
        Self::new(false, de.address, de.value, de.datum, de.script)
    }
}

impl MemoizedTransactionOutput {
    /// In-memory construction, used for tests. Re-encodes once to compute the on-wire size.
    /// Decoded values populate `original_size` directly from the input position, avoiding the re-encode.
    pub fn new(
        is_legacy: bool,
        address: Address,
        value: MemoizedValue,
        datum: MemoizedDatum,
        script: Option<MemoizedScript>,
    ) -> Self {
        let mut output = Self { original_size: 0, is_legacy, address, value, datum, script };
        output.original_size = to_cbor(&output).len();
        output
    }

    pub fn original_size(&self) -> usize {
        self.original_size
    }

    pub fn delegate(&self) -> Option<StakeCredential> {
        match &self.address {
            Address::Shelley(shelley) => match shelley.delegation() {
                ShelleyDelegationPart::Key(key) => Some(StakeCredential::AddrKeyhash(*key)),
                ShelleyDelegationPart::Script(script) => Some(StakeCredential::ScriptHash(*script)),
                ShelleyDelegationPart::Pointer(..) | ShelleyDelegationPart::Null => None,
            },
            Address::Byron(..) => None,
            Address::Stake(..) => unreachable!("stake address inside output?"),
        }
    }
}

impl<'b, C> cbor::Decode<'b, C> for MemoizedTransactionOutput {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let data_type = d.datatype()?;
        let start_pos = d.position();

        let mut output = if matches!(data_type, cbor::Type::MapIndef | cbor::Type::Map) {
            decode_modern_output(d, ctx)?
        } else if matches!(data_type, cbor::Type::ArrayIndef | cbor::Type::Array) {
            decode_legacy_output(d, ctx)?
        } else {
            return Err(cbor::decode::Error::type_mismatch(data_type));
        };

        output.original_size = d.position() - start_pos;
        Ok(output)
    }
}

fn decode_legacy_output<C>(
    d: &mut cbor::Decoder<'_>,
    ctx: &mut C,
) -> Result<MemoizedTransactionOutput, cbor::decode::Error> {
    let len = d.array()?;

    Ok(MemoizedTransactionOutput {
        original_size: 0,
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
                if cbor::decode_break(d, len)? {
                    MemoizedDatum::None
                } else {
                    let datum: Legacy<MemoizedDatum> = d.decode_with(ctx)?;
                    if !cbor::decode_break(d, len)? {
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
    let (address, value, datum, script) = cbor::heterogeneous_map(
        d,
        (None, None, MemoizedDatum::None, None),
        |d| d.u8(),
        |d, state, field| {
            match field {
                0 => state.0 = Some(decode_address(d.bytes()?)?),
                1 => state.1 = Some(d.decode_with(ctx)?),
                2 => state.2 = d.decode_with(ctx)?,
                3 => state.3 = Some(decode_script(d, ctx)?),
                _ => return cbor::unexpected_field::<MemoizedTransactionOutput, _>(field),
            }
            Ok(())
        },
    )?;

    Ok(MemoizedTransactionOutput {
        original_size: 0,
        is_legacy: false,
        address: address.ok_or_else(|| cbor::missing_field::<MemoizedTransactionOutput, Address>(0))?,
        value: value.ok_or_else(|| cbor::missing_field::<MemoizedTransactionOutput, Value>(1))?,
        datum,
        script,
    })
}

fn decode_address(address_bytes: &[u8]) -> Result<Address, cbor::decode::Error> {
    Address::from_bytes(address_bytes).map_err(|e| cbor::decode::Error::message(format!("invalid address: {e:?}")))
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

// --------------------------------------------------------------------- Helpers

fn serialize_address<S: serde::ser::Serializer>(addr: &Address, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&addr.to_hex())
}

fn deserialize_address<'de, D: serde::de::Deserializer<'de>>(deserializer: D) -> Result<Address, D::Error> {
    let bytes: &str = serde::Deserialize::deserialize(deserializer)?;
    Address::from_hex(bytes).map_err(serde::de::Error::custom)
}

// FIXME: Eventually allow serializing complete values, not just coins.
fn serialize_value<S: serde::ser::Serializer>(value: &MemoizedValue, serializer: S) -> Result<S::Ok, S::Error> {
    match value.as_ref() {
        Value::Coin(coin) => serializer.serialize_u64(*coin),
        Value::Multiasset(coin, _) => serializer.serialize_u64(*coin),
    }
}

fn deserialize_value<'de, D: serde::de::Deserializer<'de>>(deserializer: D) -> Result<MemoizedValue, D::Error> {
    #[derive(serde::Deserialize)]
    enum ValueHelper {
        Coin(u64),
        Multiasset(u64, Vec<(String, Vec<(String, u64)>)>),
    }

    let helper: ValueHelper = serde::Deserialize::deserialize(deserializer)?;

    let value = match helper {
        ValueHelper::Coin(coin) => Value::Coin(coin),
        ValueHelper::Multiasset(coin, multiasset_data) => {
            let mut converted_multiasset = Vec::new();

            for (policy_id, assets) in multiasset_data {
                let policy_id = hex::decode(&policy_id)
                    .map_err(|_| serde::de::Error::custom(format!("invalid hex string: {policy_id}")))?;

                let mut converted_assets = Vec::new();
                for (asset_name, quantity) in assets {
                    let asset_name = hex::decode(&asset_name)
                        .map_err(|_| serde::de::Error::custom(format!("invalid hex string: {asset_name}")))?;

                    converted_assets.push((
                        Bytes::from(asset_name),
                        quantity
                            .try_into()
                            .map_err(|_| serde::de::Error::custom(format!("invalid quantity value: {quantity}")))?,
                    ));
                }

                let policy_id: Hash<CREDENTIAL> = Hash::from(policy_id.as_slice());

                let pairs = NonEmptyKeyValuePairs::try_from(converted_assets)
                    .map_err(|e| serde::de::Error::custom(format!("invalid asset bundle: {e}")))?
                    .as_pallas();

                converted_multiasset.push((policy_id, pairs));
            }

            let multiasset: Multiasset<PositiveCoin> =
                Multiasset::from_vec(converted_multiasset).ok_or(serde::de::Error::custom("empty multiasset"))?;
            Value::Multiasset(coin, multiasset)
        }
    };

    MemoizedValue::new(value).map_err(serde::de::Error::custom)
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
        None::<super::PlaceholderScript> => Ok(None),
        Some(placeholder) => Ok(Some(MemoizedScript::try_from(placeholder).map_err(serde::de::Error::custom)?)),
    }
}

#[cfg(test)]
mod tests {
    use proptest::{option, prelude::*};

    use super::*;
    use crate::{
        Hash, any_hash32, any_shelley_address,
        cbor::{self, Encode},
    };

    fn any_value() -> impl Strategy<Value = MemoizedValue> {
        any::<u64>().prop_map(|coin| MemoizedValue::new(Value::Coin(coin)).expect("Value encoding should never fail"))
    }

    fn any_datum() -> impl Strategy<Value = MemoizedDatum> {
        prop_oneof![Just(MemoizedDatum::None), any_hash32().prop_map(MemoizedDatum::Hash)]
    }

    fn any_modern_output() -> impl Strategy<Value = MemoizedTransactionOutput> {
        (any_shelley_address(), any_value(), any_datum())
            .prop_map(|(address, value, datum)| MemoizedTransactionOutput::new(false, address, value, datum, None))
    }

    fn any_legacy_output() -> impl Strategy<Value = MemoizedTransactionOutput> {
        (any_shelley_address(), any_value(), option::of(any_hash32().prop_map(MemoizedDatum::Hash))).prop_map(
            |(address, value, datum_opt)| {
                MemoizedTransactionOutput::new(true, address, value, datum_opt.unwrap_or(MemoizedDatum::None), None)
            },
        )
    }

    #[test]
    fn test_encode_decode_output_with_datum_hash() {
        let hash_bytes = [1u8; 32];
        let datum_hash = Hash::<32>::from(hash_bytes);

        let datum = MemoizedDatum::Hash(datum_hash);

        let original = MemoizedTransactionOutput::new(
            false,
            Address::from_hex("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335").unwrap(),
            MemoizedValue::new(Value::Coin(1500000)).unwrap(),
            datum,
            None,
        );

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

        let original = MemoizedTransactionOutput::new(
            true,
            Address::from_hex("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335").unwrap(),
            MemoizedValue::new(Value::Coin(1500000)).unwrap(),
            datum,
            None,
        );

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
        let original = MemoizedTransactionOutput::new(
            false,
            Address::from_hex("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335").unwrap(),
            MemoizedValue::new(Value::Coin(1500000)).unwrap(),
            MemoizedDatum::None,
            None,
        );

        let mut encoder = cbor::Encoder::new(Vec::new());
        let mut ctx = ();
        original.encode(&mut encoder, &mut ctx).unwrap();
        let encoded_bytes = encoder.writer().clone();

        let mut decoder = cbor::Decoder::new(&encoded_bytes);
        let decoded: MemoizedTransactionOutput = decoder.decode_with(&mut ctx).unwrap();

        assert_eq!(original, decoded);
    }

    proptest! {
        #[test]
        fn decoded_size_matches_re_encoded_len_modern(output in any_modern_output()) {
            let bytes = to_cbor(&output);
            let decoded: MemoizedTransactionOutput = crate::from_cbor(&bytes).unwrap();
            prop_assert_eq!(decoded.original_size(), bytes.len());
            prop_assert_eq!(decoded.original_size(), output.original_size());
        }

        #[test]
        fn decoded_size_matches_re_encoded_len_legacy(output in any_legacy_output()) {
            let bytes = to_cbor(&output);
            let decoded: MemoizedTransactionOutput = crate::from_cbor(&bytes).unwrap();
            prop_assert_eq!(decoded.original_size(), bytes.len());
            prop_assert_eq!(decoded.original_size(), output.original_size());
        }
    }
}
