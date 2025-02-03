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

use amaru_ouroboros::protocol::Point;

pub(crate) mod daemon;
pub(crate) mod import;
pub(crate) mod import_chain_db;

/// Default path to the on-disk ledger storage.
pub(crate) const DEFAULT_LEDGER_DB_DIR: &str = "./ledger.db";

/// Default path to the on-disk chain storage.
pub(crate) const DEFAULT_CHAIN_DATABASE_PATH: &str = "./chain.db";

/// Default path to pre-computed on-chain data needed for block header validation.
pub(crate) const DEFAULT_DATA_DIR: &str = "./data";

/// Utility function to parse a point from a string.
///
/// Expects the input to be of the form '<point>.<hash>', where `<point>` is a number and `<hash>`
/// is a hex-encoded 32 bytes hash.
/// The first argument is the string to parse, the `bail` function is user to
/// produce the error type `E` in case of failure to parse.
pub(crate) fn parse_point<'a, F, E>(raw_str: &str, bail: F) -> Result<Point, E>
where
    F: Fn(&'a str) -> E + 'a,
{
    let mut split = raw_str.split('.');

    let slot = split
        .next()
        .ok_or(bail("missing slot number before '.'"))
        .and_then(|s| {
            s.parse::<u64>()
                .map_err(|_| bail("failed to parse point's slot as a non-negative integer"))
        })?;

    let block_header_hash = split
        .next()
        .ok_or(bail("missing block header hash after '.'"))
        .and_then(|s| {
            hex::decode(s).map_err(|_| bail("unable to decode block header hash from hex"))
        })?;

    Ok(Point::Specific(slot, block_header_hash))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_point() {
        let point = parse_point("42.0123456789abcdef", |s| s).unwrap();
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
            |s| s,
        )
        .unwrap();
        match point {
            Point::Specific(slot, _hash) => {
                assert_eq!(70070379, slot);
            }
            _ => panic!("expected a specific point"),
        }
    }
}
