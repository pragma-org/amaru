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

use crate::{
    Hash,
    size::{CREDENTIAL, KEY, SCRIPT},
};

// TODO: Unify with StakeCredential
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub enum StakePayload {
    Script(Hash<{ SCRIPT }>),
    Key(Hash<{ KEY }>),
}

impl StakePayload {
    pub fn from_script_hash(hash: Hash<{ SCRIPT }>) -> Self {
        Self::Script(hash)
    }

    pub fn from_key_hash(hash: Hash<{ KEY }>) -> Self {
        Self::Key(hash)
    }

    pub fn is_script(&self) -> bool {
        matches!(self, Self::Script(_))
    }

    /// Get a reference to the inner hash of this address part
    pub fn as_hash(&self) -> &Hash<{ CREDENTIAL }> {
        match self {
            Self::Key(h) | Self::Script(h) => h,
        }
    }
}

impl AsRef<[u8]> for StakePayload {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Key(h) | Self::Script(h) => h.as_ref(),
        }
    }
}
