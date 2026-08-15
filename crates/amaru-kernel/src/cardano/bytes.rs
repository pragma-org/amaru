// Copyright 2026 PRAGMA
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

use std::{fmt, ops::Deref, str::FromStr};

use amaru_minicbor_extra::decode_bytes;

use crate::cbor;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String")]
#[serde(try_from = "String")]
pub struct Bytes(cbor::bytes::ByteVec);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Bytes").field(&hex::encode(self.0.as_slice())).finish()
    }
}

impl<C> cbor::Encode<C> for Bytes {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.bytes(self.0.as_slice())?.ok()
    }
}

impl<'d, C> cbor::Decode<'d, C> for Bytes {
    fn decode(d: &mut cbor::Decoder<'d>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(Bytes(cbor::bytes::ByteVec::from(decode_bytes(d)?.into_owned())))
    }
}

impl Default for Bytes {
    fn default() -> Self {
        Self::from(vec![])
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(xs: Vec<u8>) -> Self {
        Bytes(cbor::bytes::ByteVec::from(xs))
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(b: Bytes) -> Self {
        b.0.into()
    }
}

impl Deref for Bytes {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<const N: usize> TryFrom<&Bytes> for [u8; N] {
    type Error = core::array::TryFromSliceError;

    fn try_from(value: &Bytes) -> Result<Self, Self::Error> {
        value.0.as_slice().try_into()
    }
}

impl TryFrom<String> for Bytes {
    type Error = hex::FromHexError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let v = hex::decode(value)?;
        Ok(Bytes(cbor::bytes::ByteVec::from(v)))
    }
}

impl FromStr for Bytes {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let v = hex::decode(s)?;
        Ok(Bytes(cbor::bytes::ByteVec::from(v)))
    }
}

impl From<Bytes> for String {
    fn from(b: Bytes) -> Self {
        hex::encode(b.deref())
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes: Vec<u8> = self.clone().into();

        f.write_str(&hex::encode(bytes))
    }
}
