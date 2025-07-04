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

use crate::{cbor, memoized::blanket_try_from_hex_bytes, Bytes, Hash, Hasher, KeepRaw, PlutusData};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "&str")]
#[serde(into = "String")]
pub struct MemoizedPlutusData {
    original_bytes: Bytes,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'data' should be reflected onto the 'original_bytes'.
    data: PlutusData,
}

impl MemoizedPlutusData {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn hash(&self) -> Hash<32> {
        Hasher::<256>::hash(&self.original_bytes)
    }
}

impl From<MemoizedPlutusData> for String {
    fn from(plutus_data: MemoizedPlutusData) -> Self {
        hex::encode(&plutus_data.original_bytes[..])
    }
}

impl<'b> From<KeepRaw<'b, PlutusData>> for MemoizedPlutusData {
    fn from(data: KeepRaw<'b, PlutusData>) -> Self {
        Self {
            original_bytes: Bytes::from(data.raw_cbor().to_vec()),
            data: data.unwrap(),
        }
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
