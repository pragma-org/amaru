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

//! Value serialization following the Haskell cardano-ledger's encoding conventions.
//!
//! The `OutputTooBigUTxO` rule does not measure the bytes found on the wire: cardano-ledger
//! re-serializes the value with its own encoder and compares that length against `maxValueSize`.
//! That encoder writes maps of more than 23 entries with an indefinite-length header and a break byte,
//! which is one byte shorter than the definite-length encoding used once a map holds 256 entries or more.
//!
use std::convert::Infallible;

use amaru_kernel::{Value, cbor};

/// The size in bytes of the value as cardano-ledger serializes it for the `OutputTooBigUTxO` rule.
pub(super) fn cardano_node_value_size(value: &Value) -> usize {
    let mut encoder = cbor::Encoder::new(ByteCounter::default());
    #[expect(clippy::expect_used)]
    encode_value(&mut encoder, value).expect("writing to a byte counter cannot fail");
    encoder.into_writer().length
}

/// A CBOR sink that only counts bytes
#[derive(Default)]
struct ByteCounter {
    length: usize,
}

impl cbor::encode::Write for ByteCounter {
    type Error = Infallible;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.length += buf.len();
        Ok(())
    }
}

fn encode_value<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    value: &Value,
) -> Result<(), cbor::encode::Error<W::Error>> {
    match value {
        Value::Coin(coin) => {
            e.u64(*coin)?;
        }
        Value::Multiasset(coin, multiasset) => {
            e.array(2)?.u64(*coin)?;
            ledger_map(e, multiasset.len(), multiasset.iter(), |e, (policy, assets)| {
                e.bytes(policy.as_ref())?;
                ledger_map(e, assets.len(), assets.iter(), |e, (name, quantity)| {
                    e.bytes(name.as_ref())?.u64(u64::from(*quantity))?;
                    Ok(())
                })
            })?;
        }
    }
    Ok(())
}

/// Write a map the way cardano-ledger does: definite-length up to 23 entries, indefinite-length
/// (header plus break byte) above.
fn ledger_map<W: cbor::encode::Write, I>(
    e: &mut cbor::Encoder<W>,
    entries: usize,
    items: impl Iterator<Item = I>,
    mut each: impl FnMut(&mut cbor::Encoder<W>, I) -> Result<(), cbor::encode::Error<W::Error>>,
) -> Result<(), cbor::encode::Error<W::Error>> {
    if entries <= 23 {
        e.map(entries as u64)?;
    } else {
        e.begin_map()?;
    }
    for item in items {
        each(e, item)?;
    }
    if entries > 23 {
        e.end()?;
    }
    Ok(())
}
