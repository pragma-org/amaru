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

/// A newtype wrapper meant to facilitate CBOR encoding of time::Duration as integers with
/// pico-precision. This may seem odd, but is necessary to mimic the encoding behavior of *some*
/// Haskell types such as 'RelativeTime'.
///
/// Note that the Haskell RelativeTime truncates to whole seconds, which this helper does not.
/// Haskell should be able to read values encoded by this helper, but will truncate to whole seconds
/// when computing with the values.
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
        let s = self.0.as_nanos();
        match s.checked_mul(1000).and_then(|s| u64::try_from(s).ok()) {
            Some(t) => t.encode(e, ctx),
            None => {
                e.tag(cbor::IanaTag::PosBignum)?;
                e.bytes(&(BigUint::from(s) * 1000u16).to_bytes_be())?;
                Ok(())
            }
        }
    }
}

impl<'b, C> cbor::Decode<'b, C> for SerialisedAsPico {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        use cbor::Type::*;
        match d.datatype()? {
            Tag => {
                cbor::expect_tag(d, cbor::IanaTag::PosBignum)?;
                let nanos = BigUint::from_bytes_be(d.bytes()?) / 1000u16;
                match u128::try_from(nanos) {
                    Ok(nanos) => {
                        if nanos < (u64::MAX as u128) * 1_000_000_000 {
                            Ok(Self(Duration::from_nanos_u128(nanos)))
                        } else {
                            Err(cbor::decode::Error::message(format!(
                                "cannot convert to Duration, too large: {nanos}ns"
                            )))
                        }
                    }
                    Err(nanos) => Err(cbor::decode::Error::message(format!(
                        "cannot convert to Duration, too large: {}ns",
                        nanos.into_original()
                    ))),
                }
            }
            U64 | U32 | U16 | U8 => Ok(Self(Duration::from_nanos(d.u64()? / 1000))),
            t => Err(cbor::decode::Error::message(format!(
                "Unhandled type decoding SerialisedAsPico: {t}"
            ))),
        }
    }
}
