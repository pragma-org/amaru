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

use crate::{Hash, size::NONCE};

pub type Nonce = Hash<NONCE>;

/// Utility function to parse a nonce (i.e. a blake2b-256 hash digest) from an hex-encoded string.
pub fn parse_nonce(hex_str: &str) -> Result<Nonce, String> {
    hex::decode(hex_str)
        .map_err(|e| format!("invalid hex encoding: {e}"))
        .and_then(|bytes| {
            <[u8; 32]>::try_from(bytes).map_err(|_| "expected 32-byte nonce".to_string())
        })
        .map(Nonce::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nonce() {
        assert!(matches!(
            parse_nonce("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d"),
            Ok(..)
        ));
    }

    #[test]
    fn test_parse_nonce_not_hex() {
        assert!(matches!(parse_nonce("patate"), Err(..)));
    }

    #[test]
    fn test_parse_nonce_too_long() {
        assert!(matches!(
            parse_nonce("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d1234"),
            Err(..)
        ));
    }

    #[test]
    fn test_parse_nonce_too_short() {
        assert!(matches!(
            parse_nonce("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a"),
            Err(..)
        ));
    }
}
