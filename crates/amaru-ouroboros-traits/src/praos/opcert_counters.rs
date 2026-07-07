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

use std::collections::BTreeMap;

use amaru_kernel::{PoolId, cbor, cbor as minicbor};

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode,
)]
#[cbor(transparent)]
pub struct OpcertCounters {
    #[n(0)]
    counters: BTreeMap<PoolId, u64>,
}

impl OpcertCounters {
    pub fn get(&self, pool_id: &PoolId) -> Option<u64> {
        self.counters.get(pool_id).cloned()
    }

    pub fn insert(&mut self, pool_id: PoolId, counter: u64) {
        self.counters.insert(pool_id, counter);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PoolId, &u64)> + '_ {
        self.counters.iter()
    }
}

impl From<BTreeMap<PoolId, u64>> for OpcertCounters {
    fn from(counters: BTreeMap<PoolId, u64>) -> Self {
        Self { counters }
    }
}
