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

use crate::{BoundedBytes, Ed25519Signature, VerificationKey, cbor};

pub const CHAIN_CODE_SIZE: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct BootstrapWitness {
    #[n(0)]
    pub public_key: VerificationKey,

    #[n(1)]
    pub signature: Ed25519Signature,

    /// FIXME(cbor): this is era-dependent. V11 is unbounded but V12 is bounded to 32 bytes.
    #[n(2)]
    pub chain_code: BoundedBytes,

    #[n(3)]
    pub attributes: BoundedBytes,
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{ByronAddress, Hash, hash, include_cbor};

    macro_rules! fixture {
        ($hash:literal) => {
            (include_cbor!(concat!("bootstrap_witnesses/", $hash, ".cbor")), hash!($hash))
        };
    }

    #[test_case(fixture!("232b6238656c07529e08b152f669507e58e2cb7491d0b586d9dbe425"))]
    #[test_case(fixture!("323ea3dd5b510b1bd5380b413477179df9a6de89027fd817207f32c6"))]
    #[test_case(fixture!("59f44fd32ee319bcea9a51e7b84d7c4cb86f7b9b12f337f6ca9e9c85"))]
    #[test_case(fixture!("65b1fe57f0ed455254aacf1486c448d7f34038c4c445fa905de33d8e"))]
    #[test_case(fixture!("a5a8b29a838ce9525ce6c329c99dc89a31a7d8ae36a844eef55d7eb9"))]
    fn to_root_key_hash((bootstrap_witness, root): (BootstrapWitness, Hash<28>)) {
        assert_eq!(ByronAddress::root(&bootstrap_witness), root)
    }
}
