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

use std::{fmt, ops::Deref, str::FromStr};

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy, any};

use crate::{
    Hash, Hasher, bech32,
    cardano::fixed_bytes::FixedBytes,
    cbor,
    hash::{InvalidHashSize, size::POOL_COLD_KEY},
};

/// Identifier for a stake pool: the hash of its cold verification key.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    std::hash::Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(transparent)]
#[schemars(transparent)]
#[repr(transparent)]
pub struct PoolId(Hash<{ POOL_COLD_KEY }>);

impl PoolId {
    #[inline]
    pub const fn new(bytes: [u8; POOL_COLD_KEY]) -> Self {
        Self(Hash::new(bytes))
    }

    pub fn to_bech32(&self) -> String {
        bech32::encode(*bech32::HRP_POOL, self)
            .unwrap_or_else(|| unreachable!("pool id can always be encoded to bech32"))
    }
}

impl From<FixedBytes<32>> for PoolId {
    #[inline]
    fn from(cold: FixedBytes<32>) -> Self {
        Self::from(Hasher::<{ 8 * POOL_COLD_KEY }>::hash(cold.as_slice()))
    }
}

impl From<Hash<{ POOL_COLD_KEY }>> for PoolId {
    #[inline]
    fn from(hash: Hash<{ POOL_COLD_KEY }>) -> Self {
        Self(hash)
    }
}

impl From<PoolId> for Hash<{ POOL_COLD_KEY }> {
    #[inline]
    fn from(pool: PoolId) -> Self {
        pool.0
    }
}

impl From<[u8; POOL_COLD_KEY]> for PoolId {
    #[inline]
    fn from(bytes: [u8; POOL_COLD_KEY]) -> Self {
        Self::new(bytes)
    }
}

impl TryFrom<&[u8]> for PoolId {
    type Error = InvalidHashSize;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Hash::try_from(value).map(Self)
    }
}

impl AsRef<[u8]> for PoolId {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Deref for PoolId {
    type Target = Hash<{ POOL_COLD_KEY }>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_bech32())
    }
}

impl fmt::Debug for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PoolId").field(&hex::encode(self)).finish()
    }
}

impl FromStr for PoolId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Hash::from_str(s).map(Self)
    }
}

impl<C> cbor::Encode<C> for PoolId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'a, C: cbor::HasProtocolVersion> cbor::Decode<'a, C> for PoolId {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.decode_with(ctx).map(Self)
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for PoolId {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        any::<Hash<{ POOL_COLD_KEY }>>().prop_map(Self).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "0f292fcaa02b8b2f9b3c8f9fd8e0bb21abedb692a6d5058df3ef2735";

    #[test]
    fn display_is_bech32() {
        let pool = PoolId::from_str(HEX).expect("hex");
        assert_eq!(pool.to_string(), "pool1pu5jlj4q9w9jlxeu370a3c9myx47md5j5m2str0naunn2q3lkdy");
    }

    #[test]
    fn json_is_hex_string() {
        let pool = PoolId::from_str(HEX).expect("hex");
        let json = serde_json::to_string(&pool).expect("json");
        assert_eq!(json, format!("\"{HEX}\""));
        assert_eq!(pool, serde_json::from_str::<PoolId>(&json).expect("parse"));
    }

    #[test]
    fn cbor_matches_inner_hash() {
        let pool = PoolId::from_str(HEX).expect("hex");
        assert_eq!(crate::to_cbor(&pool), crate::to_cbor(&*pool));
    }
}
