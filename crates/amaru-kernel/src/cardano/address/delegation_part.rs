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
    AddressPointer, Hash, bech32,
    size::{CREDENTIAL, KEY, SCRIPT},
};

/// The delegation part of a Shelley address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub enum ShelleyDelegationPart {
    Key(Hash<{ KEY }>),
    Script(Hash<{ SCRIPT }>),
    Pointer(AddressPointer),
    Null,
}

impl ShelleyDelegationPart {
    pub fn is_script(&self) -> bool {
        matches!(self, Self::Script(_))
    }

    pub fn from_key_hash(hash: Hash<{ KEY }>) -> Self {
        Self::Key(hash)
    }

    pub fn from_script_hash(hash: Hash<{ SCRIPT }>) -> Self {
        Self::Script(hash)
    }

    pub fn try_from_pointer(bytes: &[u8]) -> Option<Self> {
        AddressPointer::parse(bytes).map(Self::Pointer)
    }

    /// Get a reference to the inner hash of this address part
    pub fn as_hash(&self) -> Option<&Hash<{ CREDENTIAL }>> {
        match self {
            Self::Key(h) | Self::Script(h) => Some(h),
            Self::Pointer(_) | Self::Null => None,
        }
    }

    pub fn to_vec(&self) -> Vec<u8> {
        match self {
            Self::Key(h) | Self::Script(h) => h.to_vec(),
            Self::Pointer(ptr) => ptr.to_vec(),
            Self::Null => vec![],
        }
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.to_vec())
    }

    pub fn to_bech32(&self) -> Option<String> {
        let hrp = match self {
            Self::Key(_) => Some(*bech32::HRP_STAKE_VKH),
            Self::Script(_) => Some(*bech32::HRP_STAKE_SHARED_VKH),
            Self::Pointer(..) | Self::Null => None,
        }?;

        Some(
            bech32::encode(hrp, self.to_vec())
                .unwrap_or_else(|| unreachable!("key or script can always be encoded to bech32")),
        )
    }
}
