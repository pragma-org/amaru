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
use std::{env, num::ParseIntError};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[repr(transparent)]
pub struct NetworkMagic(u64);

impl From<u64> for NetworkMagic {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<NetworkMagic> for u64 {
    fn from(value: NetworkMagic) -> Self {
        value.0
    }
}

impl NetworkMagic {
    pub const TESTNET: Self = Self(1097911063);
    pub const MAINNET: Self = Self(764824073);
    pub const PREVIEW: Self = Self(2);
    pub const PREPROD: Self = Self(1);

    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn for_testing() -> Self {
        env::var("NETWORK_MAGIC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(Self::MAINNET)
    }
}

impl std::str::FromStr for NetworkMagic {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "testnet" => Ok(Self::TESTNET),
            "mainnet" => Ok(Self::MAINNET),
            "preview" => Ok(Self::PREVIEW),
            "preprod" => Ok(Self::PREPROD),
            _ => Ok(Self(s.parse()?)),
        }
    }
}

impl<C> Encode<C> for NetworkMagic {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for NetworkMagic {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, decode::Error> {
        u64::decode(d, ctx).map(NetworkMagic)
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::prop_cbor_roundtrip;
    use proptest::prelude::{Just, Strategy};
    use proptest::prop_oneof;

    prop_cbor_roundtrip!(NetworkMagic, any_network_magic());

    pub fn any_network_magic() -> impl Strategy<Value = NetworkMagic> {
        prop_oneof![
            1 => Just(NetworkMagic::MAINNET),
            1 => Just(NetworkMagic::PREVIEW),
            1 => Just(NetworkMagic::PREPROD),
            1 => Just(NetworkMagic::TESTNET),
        ]
    }
}
