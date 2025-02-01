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

use amaru::sync::Point;

pub(crate) mod daemon;
pub(crate) mod import;

/// Default path to the on-disk ledger storage.
pub(crate) const DEFAULT_LEDGER_DB_DIR: &str = "./ledger.db";

/// Default path to the on-disk chain storage.
pub(crate) const DEFAULT_CHAIN_DATABASE_PATH: &str = "./chain.db";

/// Default path to pre-computed on-chain data needed for block header validation.
pub(crate) const DEFAULT_DATA_DIR: &str = "./data";

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
