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

/// Similar to [`crate::state::volatile::IterPools`], but for accounts; It provides an unordered
/// iterator over recently unregistered accounts in an epoch that patches a read-only stable store
/// with pending updates such as registrations or de-registrations.
pub(crate) struct IterUnreachableAccounts<'volatile, F, I> {
    account_exists: Box<F>,
    iter_recently_unregistered_accounts: Option<I>,
    registrations: BTreeSet<&'volatile StakeCredential>,
    deregistrations: BTreeSet<&'volatile StakeCredential>,
    pools_rewards_accounts: BTreeSet<StakeCredential>,
}

impl<'volatile, F, I> IterUnreachableAccounts<'volatile, F, I>
where
    F: Fn(&StakeCredential) -> bool,
    I: Iterator<Item = StakeCredential>,
{
    pub fn new(
        account_exists: F,
        iter_recently_unregistered_accounts: I,
        registrations: &mut BTreeSet<&'volatile StakeCredential>,
        deregistrations: &mut BTreeSet<&'volatile StakeCredential>,
        pools_rewards_accounts: BTreeSet<StakeCredential>,
    ) -> Self {
        Self {
            account_exists: Box::new(account_exists),
            iter_recently_unregistered_accounts: Some(iter_recently_unregistered_accounts),
            registrations: mem::take(registrations),
            deregistrations: mem::take(deregistrations),
            pools_rewards_accounts,
        }
    }
}

impl<'volatile, F, I> Iterator for IterUnreachableAccounts<'volatile, F, I>
where
    F: Fn(&StakeCredential) -> bool,
    I: Iterator<Item = StakeCredential>,
{
    type Item = StakeCredential;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(db_iterator) = self.iter_recently_unregistered_accounts.as_mut() {
            for account in db_iterator {
                self.pools_rewards_accounts.remove(&account);

                if self.registrations.contains(&account) {
                    continue;
                }

                return Some(account);
            }

            if let Some(account) = self.deregistrations.pop_first() {
                self.pools_rewards_accounts.remove(account);
                return Some(*account);
            }
        }

        // All recently unregistered accounts have yielded, we must now look at the remaining
        // pools rewards accounts to see if any is missing from the db.
        while let Some(account) = self.pools_rewards_accounts.pop_first() {
            // We still need to check for registrations here. If an account is there, then there's
            // no need reach for the db. It is NOT unreachable.
            if self.registrations.contains(&account) {
                continue;
            }

            if self.account_exists.as_ref()(&account) {
                // Here we need not to check for de-registrations because we have already
                // removed the recently unregistered accounts from the pools_rewards_accounts
                // just above.
                continue;
            }

            // We already check that the account was not registered recently; so if it's
            // also not in the stable store, it's definitely not there.
            return Some(account);
        }

        None
    }
}
