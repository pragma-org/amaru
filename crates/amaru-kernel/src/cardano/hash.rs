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

pub use pallas_crypto::hash::{Hash, Hasher};

// -----------------------------------------------------------------------------
// Hash sizes
// -----------------------------------------------------------------------------

pub mod size {
    pub const BLOCK_BODY: usize = 32;

    pub const CREDENTIAL: usize = 28;

    pub const DATUM: usize = 32;

    pub const HEADER: usize = 32;

    pub const KEY: usize = CREDENTIAL;

    pub const NONCE: usize = 32;

    pub const POOL_COLD_KEY: usize = 28;

    pub const SCRIPT: usize = CREDENTIAL;

    pub const TRANSACTION_BODY: usize = 32;

    pub const VRF_KEY: usize = 32;
}

// -----------------------------------------------------------------------------
// Aliases
// -----------------------------------------------------------------------------

pub type HeaderHash = Hash<{ size::HEADER }>;

pub type PoolId = Hash<{ size::POOL_COLD_KEY }>;

pub type TransactionId = Hash<{ size::TRANSACTION_BODY }>;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

pub const NULL_HASH28: Hash<28> = Hash::new([0; 28]);

pub const NULL_HASH32: Hash<32> = Hash::new([0; 32]);

pub const ORIGIN_HASH: Hash<{ size::HEADER }> = NULL_HASH32;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    pub fn any_hash28() -> impl Strategy<Value = Hash<28>> {
        any::<[u8; 28]>().prop_map(Hash::from)
    }

    pub fn any_hash32() -> impl Strategy<Value = Hash<32>> {
        any::<[u8; 32]>().prop_map(Hash::from)
    }
}
