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

use std::{cell::RefCell, fmt, num::NonZeroUsize, ops::Deref};

use bytes::Bytes;

use crate::{cbor, utils::debug_bytes};

// Newtype wrapper for custom Debug.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[repr(transparent)]
pub struct NonEmptyBytes(Bytes);

impl<'de> serde::Deserialize<'de> for NonEmptyBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = Bytes::deserialize(deserializer)?;
        Self::new(bytes).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, thiserror::Error)]
#[error("empty bytes are not allowed")]
pub struct EmptyBytesError;

impl NonEmptyBytes {
    pub fn new(bytes: Bytes) -> Result<Self, EmptyBytesError> {
        if bytes.is_empty() { Err(EmptyBytesError) } else { Ok(Self(bytes)) }
    }

    pub fn from_slice(slice: &[u8]) -> Result<Self, EmptyBytesError> {
        if slice.is_empty() { Err(EmptyBytesError) } else { Ok(Self(Bytes::copy_from_slice(slice))) }
    }

    pub fn encode<T: cbor::Encode<()>>(value: &T) -> Self {
        thread_local! {
            static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        }
        #[expect(clippy::expect_used)]
        BUFFER.with_borrow_mut(|buffer| {
            cbor::encode(value, &mut *buffer).expect("serialization should not fail");
            let ret = Self::new(Bytes::copy_from_slice(buffer.as_slice())).expect("CBOR item should not be empty");
            buffer.clear();
            ret
        })
    }

    pub fn into_inner(self) -> Bytes {
        self.0
    }

    pub fn len(&self) -> NonZeroUsize {
        #[expect(clippy::expect_used)]
        NonZeroUsize::new(self.0.len()).expect("guaranteed by constructor")
    }
}

impl Deref for NonEmptyBytes {
    type Target = Bytes;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<Bytes> for NonEmptyBytes {
    type Error = EmptyBytesError;

    fn try_from(bytes: Bytes) -> Result<Self, Self::Error> {
        if bytes.is_empty() { Err(EmptyBytesError) } else { Ok(Self(bytes)) }
    }
}

impl From<NonEmptyBytes> for Bytes {
    fn from(debug_bytes: NonEmptyBytes) -> Self {
        debug_bytes.0
    }
}

impl fmt::Debug for NonEmptyBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview_hex = debug_bytes(&self.0, 32);
        let total_len = self.0.len();
        write!(f, "Bytes({total_len}, {preview_hex})")
    }
}
