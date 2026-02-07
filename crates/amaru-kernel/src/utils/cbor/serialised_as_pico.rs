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

use crate::cbor;
use num::BigUint;
use std::time::Duration;

/// Scaling factor between seconds and picoseconds
const SECONDS_TO_PICO: u64 = 1_000_000_000_000u64;

/// A newtype wrapper meant to facilitate CBOR encoding of time::Duration as integers with
/// pico-precision. This may seem odd, but is necessary to mimicks the encoding behavior of *some*
/// Haskell types such as 'RelativeTime'.
///
/// /!\ IMPORTANT
/// There is a loss of precision coming from this type when durations are below seconds.
///
/// TODO: Maybe consider promoting this as `RelativeTime`, for robustness and to avoid some
/// confusing naming...
#[derive(Debug)]
#[repr(transparent)]
pub struct SerialisedAsPico(Duration);

impl From<SerialisedAsPico> for Duration {
    fn from(t: SerialisedAsPico) -> Self {
        t.0
    }
}

impl From<Duration> for SerialisedAsPico {
    fn from(d: Duration) -> Self {
        Self(d)
    }
}

impl SerialisedAsPico {
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Duration::from_secs(serde::Deserialize::deserialize(
            deserializer,
        )?))
    }

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(&duration.as_secs(), serializer)
    }
}

impl<C> cbor::Encode<C> for SerialisedAsPico {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let s = self.0.as_secs();
        match s.checked_mul(SECONDS_TO_PICO) {
            Some(t) => t.encode(e, ctx),
            None => {
                e.tag(cbor::IanaTag::PosBignum)?;
                e.bytes(&(BigUint::from(s) * SECONDS_TO_PICO).to_bytes_be())?;
                Ok(())
            }
        }
    }
}

impl<'b, C> cbor::Decode<'b, C> for SerialisedAsPico {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        use cbor::Type::*;
        d.datatype()
            .and_then(|datatype| match datatype {
                Tag => {
                    cbor::expect_tag(d, cbor::IanaTag::PosBignum)?;
                    let s = BigUint::from_bytes_be(d.bytes()?) / SECONDS_TO_PICO;
                    u64::try_from(s).map_err(|e| {
                        cbor::decode::Error::message(format!(
                            "cannot convert to u64, too large: {e}"
                        ))
                    })
                }
                U64 | U32 | U16 | U8 => d.u64().map(|ps| ps / SECONDS_TO_PICO),
                t => Err(cbor::decode::Error::message(format!(
                    "Unhandled type decoding SerialisedAsPico: {t}"
                ))),
            })
            .map(Duration::from_secs)
            .map(Self::from)
    }
}
