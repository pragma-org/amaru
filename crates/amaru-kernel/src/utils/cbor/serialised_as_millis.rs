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

/// A newtype wrapper meant to facilitate encoding of time::Duration as integers with millis
/// precision. This may seem odd, but is necessary to mimicks the encoding behavior of *some*
/// Haskell types such as 'SlotLength'.
///
/// Note that there is a loss of precision coming from this type when durations are below
/// milliseconds. In practice, this type is used to represent seconds or tenth of seconds.
#[derive(Debug)]
#[repr(transparent)]
pub struct SerialisedAsMillis(Duration);

impl From<SerialisedAsMillis> for Duration {
    fn from(t: SerialisedAsMillis) -> Self {
        t.0
    }
}

impl From<Duration> for SerialisedAsMillis {
    fn from(d: Duration) -> Self {
        Self(d)
    }
}

impl SerialisedAsMillis {
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Duration::from_millis(serde::Deserialize::deserialize(
            deserializer,
        )?))
    }

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(&duration.as_millis(), serializer)
    }
}

impl<C> cbor::Encode<C> for SerialisedAsMillis {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let ms = self.0.as_millis();
        match u64::try_from(ms).ok() {
            Some(t) => t.encode(e, ctx),
            None => {
                let bytes = BigUint::from(ms).to_bytes_be();
                e.tag(cbor::IanaTag::PosBignum)?;
                e.bytes(&bytes)?;
                Ok(())
            }
        }
    }
}

impl<'b, C> cbor::Decode<'b, C> for SerialisedAsMillis {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        use cbor::Type::*;
        match d.datatype()? {
            Tag => {
                cbor::expect_tag(d, cbor::IanaTag::PosBignum)?;
                let millis = BigUint::from_bytes_be(d.bytes()?);
                match u128::try_from(millis) {
                    Ok(millis) => {
                        if let Some(nanos) = millis.checked_mul(1_000_000)
                            && nanos < (u64::MAX as u128) * 1_000_000_000
                        {
                            Ok(Self(Duration::from_nanos_u128(nanos)))
                        } else {
                            Err(cbor::decode::Error::message(format!(
                                "cannot convert to Duration, too large: {millis}ms"
                            )))
                        }
                    }
                    Err(millis) => Err(cbor::decode::Error::message(format!(
                        "cannot convert to Duration, too large: {}ms",
                        millis.into_original()
                    ))),
                }
            }
            U64 | U32 | U16 | U8 => Ok(Self(Duration::from_millis(d.u64()?))),
            t => Err(cbor::decode::Error::message(format!(
                "Unhandled type decoding SerialisedAsMillis: {t}"
            ))),
        }
    }
}
