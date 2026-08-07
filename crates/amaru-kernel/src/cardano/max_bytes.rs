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
/// `Eq`, `Ord` and `Hash` are written by hand rather than derived, because the array is padded past
/// `len` and a derived impl would compare the padding: `b"a"` and `b"a\0"` share the same backing
/// array and would wrongly compare equal.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(into = "String")]
#[serde(try_from = "String")]
pub struct MaxBytes<const MAX: usize> {
    bytes: [u8; MAX],
    len: usize,
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

impl<const MAX: usize> PartialEq for MaxBytes<MAX> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<const MAX: usize> Eq for MaxBytes<MAX> {}

impl<const MAX: usize> PartialOrd for MaxBytes<MAX> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<const MAX: usize> Ord for MaxBytes<MAX> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<const MAX: usize> std::hash::Hash for MaxBytes<MAX> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state)
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

impl<const N: usize> From<MaxBytes<N>> for String {
    fn from(bytes: MaxBytes<N>) -> Self {
        hex::encode(bytes.to_vec())
    }
}

impl<const N: usize> TryFrom<String> for MaxBytes<N> {
    type Error = MaxBytesError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::checked(&hex::decode(value)?)
    }
}

impl<const N: usize> FromStr for MaxBytes<N> {
    type Err = MaxBytesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

impl<const N: usize> fmt::Display for MaxBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.to_vec()))
    }
}

impl<'b, C, const N: usize> cbor::Decode<'b, C> for MaxBytes<N> {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Self::checked(&cbor::decode_bytes(d)?).map_err(|e| cbor::decode::Error::message(e.to_string()))
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
