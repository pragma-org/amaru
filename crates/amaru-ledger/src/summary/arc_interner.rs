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

use amaru_kernel::{DRep, PoolId, StakeCredential};
use std::{collections::BTreeSet, sync::Arc};

/// A structure for [interning](https://en.wikipedia.org/wiki/String_interning) recurring keys held
/// in memory. This allows to drastically reduce the number of allocated elements for keys that are
/// often found over many places.
///
/// This is particularly true for:
///
/// - Stake distributions (we hold 2 to 3 in memory usually).
/// - Rewards summary (only transient, but shared many stake credentials and pool ids with stake distr)
#[derive(Default)]
pub struct ArcInterner {
    pub accounts: hashbrown::HashSet<Arc<StakeCredential>>,
    pub pools: hashbrown::HashSet<Arc<PoolId>>,
    pub dreps: BTreeSet<Arc<DRep>>,
}

impl ArcInterner {
    /// This must be called periodically (e.g. once every epoch) to free-up previously allocated
    /// keys that are no longer used (because pool have fully unregistered, for example);
    /// otherwise, we're just leaking memory forever...
    ///
    /// Fortunately, the Arc makes this easy because they keep a count of references to them. So it
    /// suffices to look for items that have don't have references anymore and drop them.
    pub fn free_orphans(&mut self) {
        self.accounts.retain(|arc| Arc::strong_count(arc) > 0);
        self.pools.retain(|arc| Arc::strong_count(arc) > 0);
        self.dreps.retain(|arc| Arc::strong_count(arc) > 0);
    }
}

/// A common interface for any intern-able object.
pub trait ArcIntern<T> {
    fn intern(&mut self, value: T) -> Arc<T>;
}

impl ArcIntern<StakeCredential> for ArcInterner {
    fn intern(&mut self, account: StakeCredential) -> Arc<StakeCredential> {
        self.accounts.get(&account).cloned().unwrap_or_else(|| {
            let rc = Arc::new(account);
            let clone = rc.clone();
            self.accounts.insert(rc);
            clone
        })
    }
}

impl ArcIntern<PoolId> for ArcInterner {
    fn intern(&mut self, pool: PoolId) -> Arc<PoolId> {
        self.pools.get(&pool).cloned().unwrap_or_else(|| {
            let rc = Arc::new(pool);
            let clone = rc.clone();
            self.pools.insert(rc);
            clone
        })
    }
}

impl ArcIntern<DRep> for ArcInterner {
    fn intern(&mut self, drep: DRep) -> Arc<DRep> {
        self.dreps.get(&drep).cloned().unwrap_or_else(|| {
            let rc = Arc::new(drep);
            let clone = rc.clone();
            self.dreps.insert(rc);
            clone
        })
    }
}
