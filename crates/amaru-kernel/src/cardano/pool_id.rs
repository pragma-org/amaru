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

use std::fmt::Display;

use minicbor::{Decode, Decoder, Encode, Encoder, decode::Error, encode::Write};
use pallas_crypto::{
    hash::{Hash, Hasher},
    key::ed25519,
};
use serde::{Deserialize, Serialize};

use crate::size::POOL_COLD_KEY;

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct PoolId(Hash<{ POOL_COLD_KEY }>);

impl<'b, C> Decode<'b, C> for PoolId {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, Error> {
        let hash = Hash::<{ POOL_COLD_KEY }>::decode(d, ctx)?;
        Ok(Self(hash))
    }
}

impl<C> Encode<C> for PoolId {
    fn encode<W: Write>(&self, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl PoolId {
    pub fn new(hash: Hash<{ POOL_COLD_KEY }>) -> Self {
        Self(hash)
    }

    pub fn as_slice(&self) -> &[u8] {
        self.as_ref()
    }

    /// The node's cold vkey is hashed with blake2b224 to create the pool id
    pub fn from_issuer(issuer: &ed25519::PublicKey) -> Self {
        Self::new(Hasher::<{ 8 * POOL_COLD_KEY }>::hash(issuer.as_ref()))
    }
}

impl From<Hash<{ POOL_COLD_KEY }>> for PoolId {
    fn from(hash: Hash<{ POOL_COLD_KEY }>) -> Self {
        Self::new(hash)
    }
}

impl From<PoolId> for Hash<{ POOL_COLD_KEY }> {
    fn from(pool_id: PoolId) -> Self {
        pool_id.0
    }
}

impl AsRef<[u8]> for PoolId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Display for PoolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod test {
    use pallas_codec::utils::Bytes;
    use pallas_crypto::key::ed25519;

    use super::*;

    #[test]
    fn test_issuer_to_pool_id() {
        let test_vector = vec![(
            "cad3c900ca6baee9e65bf61073d900bfbca458eeca6d0b9f9931f5b1017a8cd6",
            "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
        )];

        for (issuer_vkey_str, expected_pool_id_str) in test_vector {
            let issuer_vkey: Bytes = issuer_vkey_str.parse().unwrap();
            let pool_id = PoolId::from_issuer(&ed25519::PublicKey::try_from(&issuer_vkey[..]).unwrap());
            assert_eq!(pool_id.to_string(), expected_pool_id_str);
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::PoolId;

    pub fn any_pool_id() -> impl Strategy<Value = PoolId> {
        crate::any_hash28().prop_map(PoolId::new)
    }
}
