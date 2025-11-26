// Copyright 2025 PRAGMA
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

use pallas_network::miniprotocols::txsubmission::EraTxId;
use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub struct EraTxIdOrd(EraTxId);

impl PartialEq for EraTxIdOrd {
    fn eq(&self, other: &Self) -> bool {
        self.0.0 == other.0.0 && self.0.1 == other.0.1
    }
}

impl Eq for EraTxIdOrd {}

impl PartialOrd for EraTxIdOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<EraTxIdOrd> for EraTxId {
    fn from(value: EraTxIdOrd) -> Self {
        value.0
    }
}

impl From<EraTxId> for EraTxIdOrd {
    fn from(value: EraTxId) -> Self {
        EraTxIdOrd(value)
    }
}

impl Ord for EraTxIdOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.0.0.cmp(&other.0.0) {
            Ordering::Equal => self.0.1.cmp(&other.0.1),
            other @ (Ordering::Less | Ordering::Greater) => other,
        }
    }
}

impl EraTxIdOrd {
    pub fn new(id: EraTxId) -> Self {
        Self(id)
    }
}

impl std::ops::Deref for EraTxIdOrd {
    type Target = EraTxId;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
