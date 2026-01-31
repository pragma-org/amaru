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

use crate::{BootstrapWitness, Hash, Hasher, StakeCredential, Voter, size::CREDENTIAL};
use sha3::{Digest, Sha3_256};
use std::ops::Deref;

pub trait AsHash<const SIZE: usize> {
    fn as_hash(&self) -> Hash<SIZE>;
}

impl AsHash<28> for StakeCredential {
    fn as_hash(&self) -> Hash<CREDENTIAL> {
        match self {
            Self::AddrKeyhash(hash) => *hash,
            Self::ScriptHash(hash) => *hash,
        }
    }
}

impl AsHash<28> for Voter {
    fn as_hash(&self) -> Hash<CREDENTIAL> {
        match self {
            Self::DRepKey(hash)
            | Self::DRepScript(hash)
            | Self::ConstitutionalCommitteeKey(hash)
            | Self::ConstitutionalCommitteeScript(hash)
            | Self::StakePoolKey(hash) => *hash,
        }
    }
}

impl AsHash<28> for BootstrapWitness {
    /// Construct the bootstrap root from a bootstrap witness
    fn as_hash(&self) -> Hash<CREDENTIAL> {
        // CBOR header for data that will be encoded
        let prefix: &[u8] = &[131, 0, 130, 0, 88, 64];

        let mut sha_hasher = Sha3_256::new();
        sha_hasher.update(prefix);
        sha_hasher.update(self.public_key.deref());
        sha_hasher.update(self.chain_code.deref());
        sha_hasher.update(self.attributes.deref());

        let sha_digest = sha_hasher.finalize();
        Hasher::<224>::hash(&sha_digest)
    }
}
