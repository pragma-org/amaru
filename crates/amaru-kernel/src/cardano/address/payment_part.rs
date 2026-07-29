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
    Hash, bech32,
    size::{CREDENTIAL, KEY, SCRIPT},
};

/// The payment part of a Shelley address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub enum ShelleyPaymentPart {
    Key(Hash<{ KEY }>),
    Script(Hash<{ SCRIPT }>),
}

impl ShelleyPaymentPart {
    pub fn from_key_hash(hash: Hash<{ KEY }>) -> Self {
        Self::Key(hash)
    }

    pub fn from_script_hash(hash: Hash<{ SCRIPT }>) -> Self {
        Self::Script(hash)
    }

    /// Indicates if this is the hash of a script
    pub fn is_script(&self) -> bool {
        matches!(self, Self::Script(_))
    }

    /// Get a reference to the inner hash of this address part
    pub fn as_hash(&self) -> &Hash<{ CREDENTIAL }> {
        match self {
            Self::Key(h) | Self::Script(h) => h,
        }
    }

    /// Encodes this address as a sequence of bytes
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_hash().to_vec()
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.to_vec())
    }

    pub fn to_bech32(&self) -> String {
        let hrp = match self {
            Self::Key(_) => *bech32::HRP_ADDR_VKH,
            Self::Script(_) => *bech32::HRP_STAKE_VKH,
        };

        bech32::encode(hrp, self.to_vec())
            .unwrap_or_else(|| unreachable!("key or script can always be encoded to bech32"))
    }
}
