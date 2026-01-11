// Copyright 2024 PRAGMA
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

pub use pallas_codec::utils::Bytes;
pub use pallas_crypto::{
    hash::{Hash, Hasher},
    key::ed25519,
};
pub use pallas_math::math;
pub use pallas_primitives::VrfCert;
pub use pallas_primitives::conway::OperationalCert;

pub mod kes;
pub mod praos;
mod serde_util;
pub mod vrf;

pub use amaru_ouroboros_traits::*;

pub type PoolId = Hash<28>;

pub type Lovelace = u64;

/// The node's cold vkey is hashed with blake2b224 to create the pool id
pub fn issuer_to_pool_id(issuer: &ed25519::PublicKey) -> PoolId {
    Hasher::<224>::hash(issuer.as_ref())
}

#[cfg(test)]
mod test {
    use super::issuer_to_pool_id;
    use crate::ed25519;
    use pallas_codec::utils::Bytes;

    #[test]
    fn test_issuer_to_pool_id() {
        let test_vector = vec![(
            "cad3c900ca6baee9e65bf61073d900bfbca458eeca6d0b9f9931f5b1017a8cd6",
            "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
        )];

        for (issuer_vkey_str, expected_pool_id_str) in test_vector {
            let issuer_vkey: Bytes = issuer_vkey_str.parse().unwrap();
            let pool_id =
                issuer_to_pool_id(&ed25519::PublicKey::try_from(&issuer_vkey[..]).unwrap());
            assert_eq!(pool_id.to_string(), expected_pool_id_str);
        }
    }
}
