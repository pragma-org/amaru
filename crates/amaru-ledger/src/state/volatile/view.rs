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

use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    sync::Arc,
};

use amaru_kernel::{CertificatePointer, Lovelace, PoolId, PoolParams, ProposalId, ProposalsRootsRc, StakeCredential};

use crate::{
    context::ProposalState,
    state::{
        VolatileDB,
        volatile::{DiffBind, DiffEpochReg, VolatileSequence, VolatileState, fragment::add_proposals},
    },
    store::{
        ReadStore, StoreError,
        columns::{pools::Row as Pool, *},
    },
};

mod iter_pools;
mod iter_unreachable_accounts;

// ------------------------------------------------------------------------------------ VolatileView

/// An ephemeral aggregate of multiple VolatileFragment, useful at epoch boundaries or for building
/// context.
#[derive(Debug)]
pub struct VolatileView<'volatile, 'store, DB: ReadStore> {
    db: &'store DB,
    pools: Option<DiffEpochReg<PoolId, &'volatile (PoolParams, CertificatePointer, Lovelace)>>,
    proposals: BTreeMap<&'volatile ProposalId, &'volatile Arc<ProposalState>>,
    accounts: Option<AccountVolatileView<'volatile>>,
    volatile_donations: Lovelace,
}

impl<'volatile, 'db, DB: ReadStore> VolatileView<'volatile, 'db, DB> {
    /// Obtain a view of the database, which acts as a proxy 'ReadStore' augmented with the latest
    /// volatile updates, if any. This is used in context where one needs the true latest view of
    /// the ledger; for example at the epoch boundary.
    pub fn new(volatile: &'volatile VolatileDB, stable: &'db DB) -> VolatileView<'volatile, 'db, DB> {
        let mut pools = DiffEpochReg::default();
        let mut proposals = BTreeMap::new();
        let mut accounts = DiffBind::default();

        for anchored in volatile.iter() {
            accounts.extend_refs(&anchored.fragment.accounts);

            pools.extend_derefs(&anchored.fragment.pools);

            for (k, v) in anchored.fragment.proposals.iter() {
                proposals.insert(k, v);
            }
        }

        let accounts = AccountVolatileView {
            unregistered: accounts.unregistered,
            registered: accounts
                .registered
                .into_iter()
                .filter_map(|(credential, bind)| {
                    // NOTE: only accounts that are newly registered (i.e. .value is some)
                    //
                    // Delegations-only needs not to appear here as they'll be available from the
                    // stable store.
                    bind.value.map(|_| credential)
                })
                .collect(),
        };

        Self {
            db: stable,
            accounts: Some(accounts),
            pools: Some(pools),
            proposals,
            volatile_donations: volatile.resolve_donations(),
        }
    }

    /// Provides an iterator for pools on top of the stable store, but adding any pending updates
    /// from the aggregated volatile state.
    ///
    /// IMPORTANT: Yields pools in no particular order.
    pub fn iter_pools(&mut self) -> Result<impl Iterator<Item = (PoolId, Pool)>, StoreError> {
        match mem::take(&mut self.pools) {
            None => {
                // Just being careful here. There's no reason to ever call this twice; but if it
                // ever happens, this line might save us from hours of debugging.
                unreachable!(".iter_pools() called twice on the same VolatileView! Don't do that.")
            }
            Some(mut pools) => Ok(iter_pools::IterPools::new(self.db.iter_pools()?, &mut pools)),
        }
    }

    /// Provides an iterator for proposals on top of the stable store, but adding any pending updates
    /// from the aggregated volatile state.
    ///
    /// IMPORTANT: Yields proposals in no particular order.
    pub fn iter_proposals(&self) -> Result<impl Iterator<Item = (ProposalId, proposals::Row)>, StoreError> {
        Ok(self.db.iter_proposals()?.chain(add_proposals(self.proposals.iter().map(|(k, v)| (*(*k), (*v).clone())))))
    }

    /// Provides an iterator for accounts on top of the stable store, tracking accounts that are "no
    /// longer reachable" and cannot receive rewards they've been allocated.
    ///
    /// Payability is resolved from two complementary sources, because the rewarded accounts come
    /// from two places:
    ///
    /// - Delegators: registered when the stake distribution was taken but that have since
    ///   unregistered. We track a rolling 'recently_unregistered_accounts' over the past few epochs
    ///   that allows us to recover this information.
    ///
    /// - Pool leaders: pools are paid on their pool's reward account, an arbitrary stake credential
    ///   which the protocol never requires to be registered, as named by the stake distribution the
    ///   rewards were computed from. The pool may have changed or dropped that account since, and
    ///   the credential may never have had an account at all. So such an account may be missing
    ///   despite no unregistration to have ever been recorded.
    ///
    /// IMPORTANT: Yields accounts in no particular order.
    #[expect(clippy::panic)]
    pub fn iter_unreachable_accounts(
        &mut self,
        pools_owners: BTreeSet<&'_ StakeCredential>,
    ) -> Result<impl Iterator<Item = StakeCredential>, StoreError> {
        let AccountVolatileView { mut registered, mut unregistered } = match mem::take(&mut self.accounts) {
            None => {
                // Just being careful here. There's no reason to ever call this twice; but if it
                // ever happens, this line might save us from hours of debugging.
                unreachable!(".iter_unreachable_accounts() called twice on the same VolatileView! Don't do that.")
            }
            Some(accounts) => accounts,
        };

        Ok(iter_unreachable_accounts::IterUnreachableAccounts::new(
            |account| {
                self.db
                    .account(account)
                    .unwrap_or_else(|err| panic!("unexpected database error while iterating: {err}"))
                    .is_some()
            },
            self.db.iter_recently_unregistered_accounts()?,
            &mut registered,
            &mut unregistered,
            pools_owners,
        ))
    }

    /// A view on the proposal roots; this doesn't really require any volatile update but is
    /// conveniently made available from the underlying store; to avoid having to pass both a
    /// volatile view and a stable store around every function.
    pub fn proposals_roots(&self) -> Result<ProposalsRootsRc, StoreError> {
        Ok(ProposalsRootsRc::from(self.db.proposals_roots()?))
    }

    /// The donations collected over the closing epoch: those already persisted in the stable store,
    /// plus those still sitting in volatile fragments. They are all moved into the treasury at the
    /// epoch boundary.
    pub fn donations(&self) -> Result<Lovelace, StoreError> {
        Ok(self.db.pots()?.donations + self.volatile_donations)
    }
}

// ----------------------------------------------------------------------------- AccountVolatileView

/// A simplified 'DiffBind' for accounts, specialized to just the stake credentials.
#[derive(Debug)]
struct AccountVolatileView<'volatile> {
    registered: BTreeSet<&'volatile StakeCredential>,
    unregistered: BTreeSet<&'volatile StakeCredential>,
}

#[cfg(test)]
mod test {
    use amaru_kernel::{BlockHeight, Hash, Point, Slot, Tip};

    use super::*;
    use crate::state::VolatileFragment;

    /// A pool's reward account may never have been registered, in which case no unregistration was
    /// ever recorded for it. It must still be reported as unclaimed.
    #[test]
    fn pool_reward_accounts_without_an_account_are_unclaimed() {
        let stable = Stable { accounts: BTreeSet::from([credential(1)]), recently_unregistered: BTreeSet::new() };
        let volatile = VolatileDB::default();
        let mut view = VolatileView::new(&volatile, &stable);

        let pool_reward_accounts = [credential(1), credential(2)];

        assert_eq!(
            unreachable_accounts(&mut view, pool_reward_accounts.iter().collect()),
            BTreeSet::from([credential(2)]),
            "credential(1) has an account and is payable; credential(2) never had one",
        );
    }

    /// A pool can change or drop its reward account between the stake distribution snapshot and the
    /// payout: the credential owed the leader reward is then named by no pool in the registry, and
    /// if it was deregistered long enough ago, its unregistration marker has been pruned too. As
    /// long as it is fed in as a leader reward account, it is still yielded as unreachable: neither
    /// the registry nor the unregistration index is needed to catch it.
    #[test]
    fn leader_reward_accounts_no_longer_named_by_any_pool_are_unclaimed() {
        let stable = Stable { accounts: BTreeSet::new(), recently_unregistered: BTreeSet::new() };
        let volatile = VolatileDB::default();
        let mut view = VolatileView::new(&volatile, &stable);

        let leader_reward_accounts = [credential(1)];

        assert_eq!(
            unreachable_accounts(&mut view, leader_reward_accounts.iter().collect()),
            BTreeSet::from(leader_reward_accounts),
        );
    }

    /// Accounts registered when the stake distribution was taken are covered by the unregistration
    /// index rather than by a lookup.
    #[test]
    fn recently_unregistered_accounts_are_unclaimed() {
        let stable = Stable {
            accounts: BTreeSet::from([credential(1), credential(2)]),
            recently_unregistered: BTreeSet::from([credential(2)]),
        };
        let volatile = VolatileDB::default();
        let mut view = VolatileView::new(&volatile, &stable);

        assert_eq!(unreachable_accounts(&mut view, BTreeSet::new()), BTreeSet::from([credential(2)]));
    }

    /// The volatile window overrides both sources: a re-registration makes a credential payable
    /// again even though the deregistration index still lists it and the stable store hasn't caught up,
    /// while a deregistration makes one unpayable even though the stable store still holds it.
    #[test]
    fn the_volatile_window_overrides_the_stable_verdict() {
        let stable = Stable {
            accounts: BTreeSet::from([credential(1), credential(2)]),
            recently_unregistered: BTreeSet::from([credential(3), credential(4)]),
        };

        let mut fragment = VolatileFragment::default();
        // Re-registered within the window: no longer unclaimed, despite being in the index.
        fragment.accounts.register(credential(3), 0, None, None).unwrap();
        // Registered within the window, and a pool reward account: payable despite having no stable row.
        fragment.accounts.register(credential(5), 0, None, None).unwrap();
        // Unregistered within the window: unclaimed, despite still having a stable row.
        fragment.accounts.unregister(credential(2));

        let mut volatile = VolatileDB::default();
        volatile.push_back(fragment.anchor(tip(), Hash::new([0; 28])));

        let mut view = VolatileView::new(&volatile, &stable);

        let pool_reward_accounts = [credential(1), credential(5)];

        assert_eq!(
            unreachable_accounts(&mut view, pool_reward_accounts.iter().collect()),
            BTreeSet::from([credential(2), credential(4)])
        );
    }

    // HELPERS

    /// The unclaimed credentials a view yields, gathered into a set so that the order they come in
    /// and any repetition don't matter.
    fn unreachable_accounts(
        view: &mut VolatileView<'_, '_, Stable>,
        pool_reward_accounts: BTreeSet<&StakeCredential>,
    ) -> BTreeSet<StakeCredential> {
        view.iter_unreachable_accounts(pool_reward_accounts).unwrap().collect()
    }

    fn credential(tag: u8) -> StakeCredential {
        StakeCredential::AddrKeyhash(Hash::new([tag; 28]))
    }

    fn tip() -> Tip {
        Tip::new(Point::Specific(Slot::from(1), Hash::new([0; 32])), BlockHeight::from(1))
    }

    /// A stable store holding a set of accounts and a set of recently unregistered ones.
    struct Stable {
        accounts: BTreeSet<StakeCredential>,
        recently_unregistered: BTreeSet<StakeCredential>,
    }

    impl ReadStore for Stable {
        fn account(&self, credential: &StakeCredential) -> Result<Option<accounts::Row>, StoreError> {
            Ok(self.accounts.contains(credential).then(accounts::Row::default))
        }

        fn iter_recently_unregistered_accounts(
            &self,
        ) -> Result<impl Iterator<Item = recently_unregistered_accounts::Key>, StoreError> {
            Ok(self.recently_unregistered.clone().into_iter())
        }
    }
}
