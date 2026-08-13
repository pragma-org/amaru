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

/// A list of bytes with a fixed size, known at compile time.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String")]
#[serde(try_from = "String")]
pub struct FixedBytes<const N: usize>([u8; N]);

impl<const N: usize> fmt::Debug for FixedBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FixedBytes").field(&hex::encode(self.0.as_slice())).finish()
    }
}

impl<const N: usize> FixedBytes<N> {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    fn checked(bytes: &[u8]) -> Result<Self, FixedBytesError> {
        let got = bytes.len();
        if got != N {
            return Err(FixedBytesError::IncorrectSize { expected: N, got });
        }

        let mut inner = [0u8; N];
        inner[..got].copy_from_slice(bytes);
        Ok(Self(inner))
    }

    pub const fn zeroes() -> Self {
        Self([0u8; N])
    }
}

impl<const N: usize> Deref for FixedBytes<N> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<const N: usize> TryFrom<&[u8]> for FixedBytes<N> {
    type Error = FixedBytesError;

    fn try_from(xs: &[u8]) -> Result<Self, Self::Error> {
        Self::checked(xs)
    }
}

impl<const N: usize> TryFrom<Vec<u8>> for FixedBytes<N> {
    type Error = FixedBytesError;

    fn try_from(xs: Vec<u8>) -> Result<Self, Self::Error> {
        Self::checked(&xs)
    }
}

impl<const N: usize> From<[u8; N]> for FixedBytes<N> {
    fn from(array: [u8; N]) -> Self {
        Self(array)
    }
}

impl<const N: usize> From<FixedBytes<N>> for Vec<u8> {
    fn from(bytes: FixedBytes<N>) -> Self {
        bytes.to_vec()
    }
}

impl<const N: usize> From<FixedBytes<N>> for Bytes {
    fn from(bytes: FixedBytes<N>) -> Self {
        bytes.to_vec().into()
    }
}

impl<const N: usize> From<FixedBytes<N>> for String {
    fn from(bytes: FixedBytes<N>) -> Self {
        hex::encode(bytes.to_vec())
    }
}

impl<const N: usize> TryFrom<String> for FixedBytes<N> {
    type Error = FixedBytesError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::checked(&hex::decode(value)?)
    }
}

impl<const N: usize> FromStr for FixedBytes<N> {
    type Err = FixedBytesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

impl<const N: usize> fmt::Display for FixedBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.to_vec()))
    }
}

impl<'b, C, const N: usize> cbor::Decode<'b, C> for FixedBytes<N> {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Self::checked(&cbor::decode_bytes(d)?).map_err(|e| cbor::decode::Error::message(e.to_string()))
    }
}

impl<C, const N: usize> cbor::Encode<C> for FixedBytes<N> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.bytes(self.as_slice())?.ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FixedBytesError {
    #[error("expected exactly {expected} bytes, got {got}")]
    IncorrectSize { expected: usize, got: usize },

    #[error(transparent)]
    InvalidHex(#[from] hex::FromHexError),
}
