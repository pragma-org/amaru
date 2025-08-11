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
    cbor::{self, data::IanaTag},
    memoized, DatumHash, Legacy, MemoizedPlutusData, MintedDatumOption,
};
use serde::ser::SerializeStruct;

use minicbor_extra::heterogeneous_array;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoizedDatum {
    None,
    Hash(DatumHash),
    Inline(MemoizedPlutusData),
}

impl serde::Serialize for MemoizedDatum {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MemoizedDatum::None => None::<()>.serialize(serializer),
            MemoizedDatum::Hash(hash) => {
                let mut s = serializer.serialize_struct("MemoizedDatum::Hash", 1)?;
                s.serialize_field("Hash", &hash)?;
                s.end()
            }
            MemoizedDatum::Inline(data) => {
                let mut s = serializer.serialize_struct("MemoizedDatum::Data", 1)?;
                s.serialize_field("Data", &data)?;
                s.end()
            }
        }
    }
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

impl<'b, C> cbor::Decode<'b, C> for MemoizedDatum {
    #[allow(unused_variables)] // ctx will be used in the future
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, 2, |d| {
            let datum_option = d.u8()?;
            match datum_option {
                0 => {
                    let raw = d.bytes()?;
                    if raw.len() != 32 {
                        return Err(cbor::decode::Error::message(format!(
                            "expected datum hash of length 32, got {}",
                            raw.len()
                        )));
                    }
                    Ok(MemoizedDatum::Hash(pallas_primitives::Hash::<32>::from(
                        raw,
                    )))
                }
                1 => {
                    if d.tag()? != IanaTag::Cbor.tag() {
                        return Err(cbor::decode::Error::message("unknown tag for datum tag"));
                    }
                    let plutus_data: pallas_primitives::KeepRaw<'_, pallas_primitives::PlutusData> =
                        cbor::decode(d.bytes()?)?;
                    Ok(MemoizedDatum::Inline(MemoizedPlutusData::from(plutus_data)))
                }
                _ => Err(cbor::decode::Error::message(format!(
                    "unknown datum option: {}",
                    datum_option
                ))),
            }
        })
    }
}

impl<'b, C> cbor::Decode<'b, C> for Legacy<MemoizedDatum> {
    #[allow(unused_variables)] // ctx will be used in the future
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        if d.datatype()? == cbor::data::Type::Break {
            Ok(memoized::Legacy(MemoizedDatum::None))
        } else {
            let raw = d.bytes()?;
            if raw.len() != 32 {
                return Err(cbor::decode::Error::message(format!(
                    "expected legacy datum hash of length 32, got {}",
                    raw.len()
                )));
            }
            Ok(memoized::Legacy(MemoizedDatum::Hash(
                pallas_primitives::Hash::<32>::from(raw),
            )))
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
            Some(MintedDatumOption::Data(cbor_wrap)) => Self::Inline(cbor_wrap.unwrap().into()),
        }
    }
}

impl From<Option<DatumHash>> for MemoizedDatum {
    fn from(opt: Option<DatumHash>) -> Self {
        opt.map(MemoizedDatum::Hash).unwrap_or(MemoizedDatum::None)
    }
}
