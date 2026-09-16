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

use std::{
    fmt,
    ops::{Add, Sub},
};

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy};

use crate::cbor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct BlockHeight(u64);

impl BlockHeight {
    pub fn into_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for BlockHeight {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<BlockHeight> for u64 {
    fn from(value: BlockHeight) -> Self {
        value.0
    }
}

impl AsRef<BlockHeight> for BlockHeight {
    fn as_ref(&self) -> &BlockHeight {
        self
    }
}

impl Add<u64> for BlockHeight {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl Sub<u64> for BlockHeight {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0.saturating_sub(rhs))
    }
}

impl BlockHeight {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for BlockHeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<C> cbor::Encode<C> for BlockHeight {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> cbor::Decode<'b, C> for BlockHeight {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        u64::decode(d, ctx).map(BlockHeight)
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for BlockHeight {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (1..1000u64).prop_map(BlockHeight::from).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::BlockHeight;
    use crate::prop_cbor_roundtrip;

    prop_cbor_roundtrip!(BlockHeight);
}
