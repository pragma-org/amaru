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

use crate::{from_cbor, memoized::blanket_try_from_hex_bytes, Bytes, KeepRaw, NativeScript};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "&str")]
pub struct MemoizedNativeScript {
    original_bytes: Bytes,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'expr' should be reflected onto the 'original_bytes'.
    expr: NativeScript,
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
