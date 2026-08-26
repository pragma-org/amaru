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

use crate::{Credential, Hash, Voter, size::CREDENTIAL};

pub trait AsHash<const SIZE: usize> {
    fn as_hash(&self) -> Hash<SIZE>;
}

impl AsHash<28> for Credential {
    fn as_hash(&self) -> Hash<CREDENTIAL> {
        match self {
            Self::KeyHash(hash) => *hash,
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
