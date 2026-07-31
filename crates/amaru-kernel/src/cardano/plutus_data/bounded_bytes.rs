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

use std::{fmt, ops::Deref};

use crate::cbor;

// TODO: BoundedBytes should not exists
//
// Move this as a serialisation/deserialisation helper rather than being a type that
// transpires through the type system.
/// Encode bytes as CBOR bytes in chunks of 64 bytes max.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(into = "String")]
#[serde(try_from = "String")]
pub struct BoundedBytes(Vec<u8>);

impl From<Vec<u8>> for BoundedBytes {
    fn from(xs: Vec<u8>) -> Self {
        Self(xs)
    }
}

impl From<BoundedBytes> for Vec<u8> {
    fn from(bytes: BoundedBytes) -> Self {
        bytes.0
    }
}

impl Deref for BoundedBytes {
    type Target = Vec<u8>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<String> for BoundedBytes {
    type Error = hex::FromHexError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        hex::decode(value).map(Self)
    }
}

impl From<BoundedBytes> for String {
    fn from(bytes: BoundedBytes) -> Self {
        hex::encode(bytes.deref())
    }
}

impl fmt::Display for BoundedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.deref()))
    }
}

impl<C> cbor::Encode<C> for BoundedBytes {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        // we match the haskell implementation by encoding bytestrings longer than 64
        // bytes as indefinite lists of bytes
        const CHUNK_SIZE: usize = 64;
        let bs: &Vec<u8> = self.deref();
        if bs.len() <= 64 {
            e.bytes(bs)?;
        } else {
            e.begin_bytes()?;
            for b in bs.chunks(CHUNK_SIZE) {
                e.bytes(b)?;
            }
            e.end()?;
        }
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for BoundedBytes {
    fn decode(d: &mut cbor::Decoder<'b>, _: &mut C) -> Result<Self, cbor::decode::Error> {
        let mut res = Vec::new();
        for chunk in d.bytes_iter()? {
            let bs = chunk?;
            res.extend_from_slice(bs);
        }
        Ok(BoundedBytes::from(res))
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::BoundedBytes;

    prop_compose! {
        pub fn any_bounded_bytes()(
            bytes in any::<Vec<u8>>(),
        ) -> BoundedBytes {
            BoundedBytes::from(bytes)
        }
    }
}
