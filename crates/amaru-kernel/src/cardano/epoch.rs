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

use minicbor::{Decode, Decoder, Encode};
use std::{
    fmt,
    ops::{Add, Sub},
    str::FromStr,
};

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy};

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
pub struct Epoch(u64);

impl Epoch {
    pub fn new(epoch: u64) -> Self {
        Self(epoch)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for Epoch {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (0..u64::MAX).prop_map(Epoch::from).boxed()
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Epoch {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(Epoch)
    }
}

impl From<u64> for Epoch {
    fn from(epoch: u64) -> Epoch {
        Epoch(epoch)
    }
}

impl From<Epoch> for u64 {
    fn from(epoch: Epoch) -> u64 {
        epoch.0
    }
}

impl<C> Encode<C> for Epoch {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for Epoch {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.u64().map(Epoch)
    }
}

impl Add<u64> for Epoch {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Epoch(self.0 + rhs)
    }
}

impl Sub<u64> for Epoch {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Epoch(self.0 - rhs)
    }
}

impl Epoch {
    pub fn saturating_sub(self, rhs: u64) -> Self {
        Self(self.0.saturating_sub(rhs))
    }
}

impl Sub<Epoch> for Epoch {
    type Output = u64;

    fn sub(self, rhs: Epoch) -> Self::Output {
        self.0 - rhs.0
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    prop_compose! {
        pub fn any_epoch()(epoch in any::<u64>()) -> Epoch {
            Epoch::from(epoch)
        }
    }
}
