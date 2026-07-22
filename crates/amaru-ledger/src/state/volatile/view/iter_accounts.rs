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

use std::{collections::BTreeSet, mem};

use amaru_kernel::StakeCredential;

use crate::store::columns::accounts::Row as Account;

/// Similar to [`crate::state::volatile::IterPools`], but for accounts; It provides an
/// unordered iterator over accounts that patches a read-only stable store with pending updates
/// such as registrations or de-registrations.
pub(crate) struct IterAccounts<'volatile, DBIter: Iterator<Item = (StakeCredential, Account)>> {
    db_iterator: DBIter,
    registrations: BTreeSet<&'volatile StakeCredential>,
    deregistrations: BTreeSet<&'volatile StakeCredential>,
}

impl<'volatile, DBIter: Iterator<Item = (StakeCredential, Account)>> IterAccounts<'volatile, DBIter> {
    pub fn new(
        db_iterator: DBIter,
        registrations: &mut BTreeSet<&'volatile StakeCredential>,
        deregistrations: &mut BTreeSet<&'volatile StakeCredential>,
    ) -> Self {
        Self { db_iterator, registrations: mem::take(registrations), deregistrations: mem::take(deregistrations) }
    }
}

impl<'volatile, DBIter: Iterator<Item = (StakeCredential, Account)>> Iterator for IterAccounts<'volatile, DBIter> {
    type Item = StakeCredential;

    fn next(&mut self) -> Option<Self::Item> {
        for (credential, _) in &mut self.db_iterator {
            if self.deregistrations.contains(&credential) {
                continue;
            }

            return Some(credential);
        }

        if let Some(credential) = self.registrations.pop_first() {
            return Some(credential.clone());
        }

        None
    }
}
