// Copyright 2025 PRAGMA
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

use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct VersionNumber(u64);

impl From<u64> for VersionNumber {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<VersionNumber> for u64 {
    fn from(value: VersionNumber) -> Self {
        value.0
    }
}

impl AsRef<VersionNumber> for VersionNumber {
    fn as_ref(&self) -> &VersionNumber {
        self
    }
}

impl VersionNumber {
    pub const V11: VersionNumber = VersionNumber::new(11);
    pub const V12: VersionNumber = VersionNumber::new(12);
    pub const V13: VersionNumber = VersionNumber::new(13);
    pub const V14: VersionNumber = VersionNumber::new(14);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn has_query_and_peer_sharing(self) -> bool {
        self.0 >= 11
    }
}

impl<C> Encode<C> for VersionNumber {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for VersionNumber {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, decode::Error> {
        u64::decode(d, ctx).map(VersionNumber)
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use crate::prop_cbor_roundtrip;
    use crate::protocol_messages::version_number::VersionNumber;
    use proptest::prelude::{Just, Strategy};
    use proptest::prop_oneof;

    prop_cbor_roundtrip!(VersionNumber, any_version_number());

    pub fn any_version_number() -> impl Strategy<Value = VersionNumber> {
        prop_oneof![
            1 => Just(VersionNumber::V11),
            1 => Just(VersionNumber::V12),
            1 => Just(VersionNumber::V13),
            1 => Just(VersionNumber::V14),
        ]
    }
}
