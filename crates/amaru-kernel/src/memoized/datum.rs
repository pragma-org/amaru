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

use crate::{DatumHash, MemoizedPlutusData, MintedDatumOption, cbor, cbor::data::IanaTag};
use serde::ser::SerializeStruct;

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
