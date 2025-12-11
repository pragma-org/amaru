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

use bytes::Bytes;
use std::fmt;
use std::ops::{Deref, DerefMut};

// Newtype wrapper for custom Debug.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Default, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct DebugBytes(Bytes);

impl DebugBytes {
    pub fn new(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

impl Deref for DebugBytes {
    type Target = Bytes;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DebugBytes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Bytes> for DebugBytes {
    fn from(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

impl From<DebugBytes> for Bytes {
    fn from(debug_bytes: DebugBytes) -> Self {
        debug_bytes.0
    }
}

impl fmt::Debug for DebugBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = &self.0;
        let total_len = bytes.len();
        let preview_len = 32.min(total_len);
        let preview = &bytes[0..preview_len];

        let mut preview_hex = String::with_capacity(2 * preview_len * 3);
        for &b in preview {
            const HEX_CHARS: [u8; 16] = *b"0123456789abcdef";
            preview_hex.push(HEX_CHARS[(b >> 4) as usize] as char);
            preview_hex.push(HEX_CHARS[(b & 0x0f) as usize] as char);
        }
        if preview_len < total_len {
            preview_hex.push_str("...");
        }

        write!(f, "Bytes({total_len}, {preview_hex})")
    }
}
