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

pub mod array;
pub mod cbor;
pub mod serde;
pub mod string;
#[cfg(any(test, feature = "test-utils"))]
pub mod tests;

pub fn debug_bytes(bytes: &[u8], max_len: usize) -> String {
    let total_len = bytes.len();
    let preview_len = max_len.min(total_len);
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
    preview_hex
}
