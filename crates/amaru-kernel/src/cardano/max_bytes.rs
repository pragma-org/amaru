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

use crate::{Bytes, cbor};

/// A list of bytes whose length is bounded by the given maximum.
///
/// The backing array is always zero-padded past `len` (every constructor goes through `checked` or
/// `empty`, and the fields are never mutated), so the derived `Eq`, `Ord` and `Hash` agree with
/// comparisons on `as_slice()`: equal slices imply identical `(bytes, len)` pairs, and zero being
/// the smallest byte keeps the derived lexicographic order consistent with slice order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaxBytes<const MAX: usize> {
    bytes: [u8; MAX],
    len: usize,
}

impl<const MAX: usize> serde::Serialize for MaxBytes<MAX> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&hex::encode(self.as_slice()))
        } else {
            serializer.serialize_bytes(self.as_slice())
        }
    }
}

impl<'de, const MAX: usize> serde::Deserialize<'de> for MaxBytes<MAX> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            let s = String::deserialize(deserializer)?;
            Self::from_str(&s).map_err(serde::de::Error::custom)
        } else {
            let bytes = <&[u8]>::deserialize(deserializer)?;
            Self::checked(bytes).map_err(serde::de::Error::custom)
        }
    }
}

impl<const MAX: usize> MaxBytes<MAX> {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn checked(bytes: &[u8]) -> Result<Self, MaxBytesError> {
        let got = bytes.len();
        if got > MAX {
            return Err(MaxBytesError::IncorrectSize { max: MAX, got });
        }

        let mut inner = [0u8; MAX];
        inner[..got].copy_from_slice(bytes);
        Ok(Self { bytes: inner, len: got })
    }

    pub const fn empty() -> Self {
        Self { bytes: [0u8; MAX], len: 0 }
    }
}

impl<const MAX: usize> Deref for MaxBytes<MAX> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<const MAX: usize> TryFrom<&[u8]> for MaxBytes<MAX> {
    type Error = MaxBytesError;

    fn try_from(xs: &[u8]) -> Result<Self, Self::Error> {
        Self::checked(xs)
    }
}

impl<const MAX: usize> TryFrom<Vec<u8>> for MaxBytes<MAX> {
    type Error = MaxBytesError;

    fn try_from(xs: Vec<u8>) -> Result<Self, Self::Error> {
        Self::checked(&xs)
    }
}

impl<const MAX: usize> From<MaxBytes<MAX>> for Vec<u8> {
    fn from(bytes: MaxBytes<MAX>) -> Self {
        bytes.as_slice().to_vec()
    }
}

impl<const N: usize> From<MaxBytes<N>> for Bytes {
    fn from(bytes: MaxBytes<N>) -> Self {
        bytes.to_vec().into()
    }
}

impl<const N: usize> FromStr for MaxBytes<N> {
    type Err = MaxBytesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::checked(&hex::decode(s)?)
    }
}

impl<const N: usize> fmt::Display for MaxBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.as_slice()))
    }
}

impl<'b, C: cbor::HasProtocolVersion, const N: usize> cbor::Decode<'b, C> for MaxBytes<N> {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Self::checked(&cbor::decode_bytes_with(d, ctx)?).map_err(|e| cbor::decode::Error::message(e.to_string()))
    }
}

impl<C, const N: usize> cbor::Encode<C> for MaxBytes<N> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.bytes(self.as_slice())?.ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MaxBytesError {
    #[error("expected {max} bytes, got {got}")]
    IncorrectSize { max: usize, got: usize },

    #[error(transparent)]
    InvalidHex(#[from] hex::FromHexError),
}
