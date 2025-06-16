// Copyright 2024 PRAGMA
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

use amaru_kernel::{network::NetworkName, Nonce, Point};

pub(crate) mod bootstrap;
pub(crate) mod daemon;
pub(crate) mod import_headers;
pub(crate) mod import_ledger_state;
pub(crate) mod import_nonces;

pub(crate) const DEFAULT_NETWORK: NetworkName = NetworkName::Preprod;

/// Default address to listen on for incoming connections.
pub(crate) const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:3000";

/// Utility function to parse a point from a string.
///
/// Expects the input to be of the form '<point>.<hash>', where `<point>` is a number and `<hash>`
/// is a hex-encoded 32 bytes hash.
/// The first argument is the string to parse, the `bail` function is user to
/// produce the error type `E` in case of failure to parse.
pub(crate) fn parse_point(raw_str: &str) -> Result<Point, String> {
    let mut split = raw_str.split('.');

    let slot = split
        .next()
        .ok_or("missing slot number before '.'")
        .and_then(|s| {
            s.parse::<u64>()
                .map_err(|_| "failed to parse point's slot as a non-negative integer")
        })?;

    let block_header_hash = split
        .next()
        .ok_or("missing block header hash after '.'")
        .and_then(|s| hex::decode(s).map_err(|_| "unable to decode block header hash from hex"))?;

    Ok(Point::Specific(slot, block_header_hash))
}

/// Utility function to parse a nonce (i.e. a blake2b-256 hash digest) from an hex-encoded string.
pub(crate) fn parse_nonce(hex_str: &str) -> Result<Nonce, String> {
    hex::decode(hex_str)
        .map_err(|e| format!("invalid hex encoding: {e}"))
        .and_then(|bytes| {
            <[u8; 32]>::try_from(bytes).map_err(|_| "expected 32-byte nonce".to_string())
        })
        .map(Nonce::from)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_point() {
        let point = parse_point("42.0123456789abcdef").unwrap();
        match point {
            Point::Specific(slot, hash) => {
                assert_eq!(42, slot);
                assert_eq!(vec![1, 35, 69, 103, 137, 171, 205, 239], hash);
            }
            _ => panic!("expected a specific point"),
        }
    }

    #[test]
    fn test_parse_real_point() {
        let point = parse_point(
            "70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d",
        )
        .unwrap();
        match point {
            Point::Specific(slot, _hash) => {
                assert_eq!(70070379, slot);
            }
            _ => panic!("expected a specific point"),
        }
    }

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
