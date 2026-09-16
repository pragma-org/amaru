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

use std::{fmt, ops::Add};

use crate::cbor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct ExUnits {
    #[n(0)]
    pub mem: u64,
    #[n(1)]
    pub steps: u64,
}

/// Decoded as a fixed-size record, so indefinite-length encodings are rejected before protocol version V12.
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for ExUnits {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::record_v12_indefinite(d, ctx, 2, |d, ctx| {
            Ok(ExUnits { mem: decode_ex_unit(d, ctx)?, steps: decode_ex_unit(d, ctx)? })
        })
    }
}

/// Decode one execution-unit field.
///
/// The ledger holds these as an unbounded `Natural` but refuses anything above `maxBound :: Int64`
/// at decoding time, at every protocol version, so the same ceiling applies here.
fn decode_ex_unit<C>(d: &mut cbor::Decoder<'_>, ctx: &mut C) -> Result<u64, cbor::decode::Error> {
    let value: u64 = d.decode_with(ctx)?;
    if value > i64::MAX as u64 {
        return Err(cbor::decode::Error::message("execution unit exceeds the maximum of 2^63 - 1"));
    }
    Ok(value)
}

impl Add for &ExUnits {
    type Output = ExUnits;

    fn add(self, rhs: Self) -> Self::Output {
        ExUnits { mem: self.mem + rhs.mem, steps: self.steps + rhs.steps }
    }
}

impl fmt::Display for ExUnits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{mem={}, cpu={}}}", self.mem, self.steps)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::protocol_version::PROTOCOL_VERSION_10;

    /// The ledger rejects an execution unit above `maxBound :: Int64` with "values must not exceed
    /// maxBound :: Int64", whichever of the two fields carries it.
    #[test_case(&[0x82, 0x1b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00] => matches Ok(_)  ; "mem at the ceiling")]
    #[test_case(&[0x82, 0x1b, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00] => matches Err(_) ; "mem one above the ceiling")]
    #[test_case(&[0x82, 0x00, 0x1b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff] => matches Ok(_)  ; "steps at the ceiling")]
    #[test_case(&[0x82, 0x00, 0x1b, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00] => matches Err(_) ; "steps one above the ceiling")]
    fn decode_rejects_units_above_the_int64_ceiling(bytes: &[u8]) -> Result<ExUnits, cbor::decode::Error> {
        let mut version = PROTOCOL_VERSION_10;
        cbor::from_cbor_no_leftovers_with(bytes, &mut version)
    }
}
