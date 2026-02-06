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

use minicbor::{
    Decode, Decoder, Encode,
    data::{IanaTag, Type},
};
use num::BigUint;
use std::{
    fmt,
    ops::{Add, Mul},
};

#[derive(
    Clone,
    Debug,
    Copy,
    PartialEq,
    PartialOrd,
    Ord,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    Default,
)]
#[repr(transparent)]
pub struct TimeMs(u64);

impl TimeMs {
    pub fn new(ms: u64) -> Self {
        Self(ms)
    }
}

impl fmt::Display for TimeMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

impl From<u64> for TimeMs {
    fn from(ms: u64) -> TimeMs {
        TimeMs(ms)
    }
}

impl From<TimeMs> for u64 {
    fn from(time_ms: TimeMs) -> u64 {
        time_ms.0
    }
}

impl Add<u64> for TimeMs {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        TimeMs(self.0 + rhs)
    }
}

impl Add<TimeMs> for TimeMs {
    type Output = Self;

    fn add(self, rhs: TimeMs) -> Self::Output {
        TimeMs(self.0 + rhs.0)
    }
}

impl Mul<u64> for TimeMs {
    type Output = Self;

    fn mul(self, rhs: u64) -> Self::Output {
        TimeMs(self.0 * rhs)
    }
}

impl Mul<TimeMs> for u64 {
    type Output = TimeMs;

    fn mul(self, rhs: TimeMs) -> Self::Output {
        rhs * self
    }
}

/// Scaling factor between picoseconds and milliseconds
const PICOS_IN_MILLIS: u64 = 1_000_000_000u64;

impl<C> Encode<C> for TimeMs {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        match self.0.checked_mul(PICOS_IN_MILLIS) {
            Some(time_ps) => time_ps.encode(e, ctx),
            None => {
                let bytes = (BigUint::from(self.0) * PICOS_IN_MILLIS).to_bytes_be();
                IanaTag::PosBignum.encode(e, ctx)?;
                e.bytes(&bytes)?;
                Ok(())
            }
        }
    }
}

impl<'b, C> Decode<'b, C> for TimeMs {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let datatype = d.datatype();
        (match datatype {
            Ok(Type::Tag) => {
                // consume and validate tag
                let tag = d.tag()?;
                if tag != IanaTag::PosBignum.into() {
                    return Err(minicbor::decode::Error::message(format!(
                        "Unexpected CBOR tag decoding TimeMs: {tag} (expected positive bignum)"
                    )));
                };
                // Haskell node defines timestamp as a Fixed E12
                // (wrapped in a couple of newtypes), a fixed-decimal
                // number with $10^{12}$ precision, eg. picoseconds
                // precision.  The corresponding CBOR encoding can
                // therefore overflow an unsigned 64 bits integer and
                // is then encoded as a big endian large integer with
                // a sequence of bytes.
                //
                // However, the actual precision the network cares
                // about is at most milliseconds which is why our
                // `TimeMs` is what is. We therefore need to convert
                // the number we find from a big integer and scale it
                // down accordingly.
                let bytes: &[u8] = d.bytes()?;
                let num = BigUint::from_bytes_be(bytes);
                let num_ms = num / PICOS_IN_MILLIS;
                if num_ms > BigUint::from(u64::MAX) {
                    Err(minicbor::decode::Error::message(
                        "Unsigned integer too large",
                    ))
                } else {
                    Ok(num_ms.try_into().map_err(|e| {
                        minicbor::decode::Error::message(format!(
                            "Error converting BigUint to u64 {e}"
                        ))
                    })?)
                }
            }
            Ok(Type::U64) | Ok(Type::U32) | Ok(Type::U16) | Ok(Type::U8) => {
                d.u64().map(|ps| ps / PICOS_IN_MILLIS)
            }
            Ok(t) => Err(minicbor::decode::Error::message(format!(
                "Unhandled type decoding TimeMs: {t}"
            ))),
            Err(err) => Err(err),
        })
        .map(TimeMs)
    }
}
