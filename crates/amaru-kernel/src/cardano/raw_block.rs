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

use std::{fmt, ops::Deref, sync::Arc};

/// Cheaply cloneable block bytes
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct RawBlock(Arc<[u8]>);

impl fmt::Debug for RawBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = &self.0;
        let total_len = bytes.len();
        let preview_len = 32.min(total_len);
        let preview = &bytes[0..preview_len];

        let mut preview_hex = String::with_capacity(2 * preview_len + 3);
        for &b in preview {
            const HEX_CHARS: [u8; 16] = *b"0123456789abcdef";
            preview_hex.push(HEX_CHARS[(b >> 4) as usize] as char);
            preview_hex.push(HEX_CHARS[(b & 0x0f) as usize] as char);
        }
        if preview_len < total_len {
            preview_hex.push_str("...");
        }

        write!(f, "RawBlock({total_len}, {preview_hex})")
    }
}

impl Deref for RawBlock {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&[u8]> for RawBlock {
    fn from(bytes: &[u8]) -> Self {
        Self(Arc::from(bytes))
    }
}

impl From<Box<[u8]>> for RawBlock {
    fn from(bytes: Box<[u8]>) -> Self {
        Self(Arc::from(bytes))
    }
}
