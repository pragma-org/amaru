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

use amaru_kernel::{
    CertificatePointer, ComparableProposalId, Epoch, Lovelace, PoolId, PoolParams, Proposal, ProposalPointer,
    ProposalsRootsRc, StakeCredential,
};

use crate::{
    state::{
        VolatileDB,
        volatile::{DiffBind, DiffEpochReg, VolatileSequence, fragment::add_proposals},
    },
    store::{
        ReadStore, StoreError,
        columns::{pools::Row as Pool, *},
    },
};

mod iter_pools;

// ------------------------------------------------------------------------------------ VolatileView

/// An ephemeral aggregate of multiple VolatileFragment, useful at epoch boundaries or for building
/// context.
#[derive(Debug)]
pub struct VolatileView<'volatile, 'store, DB: ReadStore> {
    epoch: Epoch,
    proposal_lifetime: u64,
    db: &'store DB,
    pools: Option<DiffEpochReg<PoolId, &'volatile (PoolParams, CertificatePointer, Lovelace)>>,
    proposals: BTreeMap<&'volatile ComparableProposalId, &'volatile Arc<(Proposal, ProposalPointer)>>,
    accounts: Option<AccountVolatileView<'volatile>>,
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
            epoch: volatile.epoch(),
            proposal_lifetime: volatile.protocol_parameters().gov_action_lifetime,
            db: stable,
            accounts: Some(accounts),
            pools: Some(pools),
            proposals,
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
    pub fn iter_proposals(&self) -> Result<impl Iterator<Item = (ComparableProposalId, proposals::Row)>, StoreError> {
        Ok(self.db.iter_proposals()?.chain(add_proposals(
            self.proposals.iter().map(|(k, v)| ((*k).clone(), (*v).clone())),
            self.epoch + self.proposal_lifetime,
        )))
    }

    /// Amongst the accounts owed a reward at the epoch boundary, those that can no longer be paid
    /// because they have no account. Their rewards go to the treasury instead.
    ///
    /// Payability is resolved from two complementary sources, because the rewarded accounts come
    /// from two places:
    ///
    /// - Delegators were, by construction, registered when the stake distribution was taken. So they
    ///   can only have become unpayable by unregistering since, which the
    ///   `recently_unregistered_accounts` index records. It is patched here with the registrations
    ///   and deregistrations still pending in the volatile window.
    ///
    /// - Pool leaders are paid on their pool's reward account, an arbitrary stake credential which
    ///   the protocol never requires to be registered: the pledge is checked against the pool's
    ///   *owners* and the performance against its *delegators*, so a pool can earn a reward payable
    ///   to a credential that has never had an account at all. No unregistration was ever recorded
    ///   for such a credential, so the `recently_unregistered_accounts` index cannot speak for it and
    ///   each `pool_reward_account` is looked up directly. There are only a few thousand pools, so
    ///   this stays cheap.
    ///
    /// The unregistration column can be arbitrarily large, so it is streamed rather than materialized.
    /// The returned iterator will be eventually filtered by the accounts that effectively received a
    /// reward in order to determine the unclaimed rewards.
    ///
    /// IMPORTANT: yields credentials in no particular order and possibly more than once. Can only be
    /// called once on a given view.
    pub fn unclaimed_reward_accounts(
        &mut self,
        pool_reward_accounts: &BTreeSet<StakeCredential>,
    ) -> Result<impl Iterator<Item = StakeCredential> + use<'volatile, 'db, DB>, StoreError> {
        let db = self.db;

        let AccountVolatileView { registered, unregistered } = match mem::take(&mut self.accounts) {
            None => {
                // Just being careful here. There's no reason to ever call this twice; but if it
                // ever happens, this line might save us from hours of debugging.
                unreachable!(".unclaimed_reward_accounts() called twice on the same VolatileView! Don't do that.")
            }
            Some(accounts) => accounts,
        };

        // Pool reward accounts that never had an account to unregister in the first place. A
        // credential freshly registered within the volatile window is payable even though the stable
        // store hasn't seen it yet; one unregistered there is already yielded below.
        let mut resolved = Vec::new();
        for credential in pool_reward_accounts {
            if !registered.contains(credential) && db.account(credential)?.is_none() {
                resolved.push(credential.clone());
            }
        }

        // Unregistered within the volatile window, so the stable store hasn't caught up yet.
        let unregistered = unregistered.into_iter().cloned().collect::<Vec<_>>();

        Ok(db
            .iter_recently_unregistered_accounts()?
            // Recently unregistered, unless they have since re-registered within the volatile window.
            .filter(move |credential| !registered.contains(credential))
            .chain(unregistered)
            .chain(resolved))
    }

    /// A view on the proposal roots; this doesn't really require any volatile update but is
    /// conveniently made available from the underlying store; to avoid having to pass both a
    /// volatile view and a stable store around every function.
    pub fn proposals_roots(&self) -> Result<ProposalsRootsRc, StoreError> {
        Ok(ProposalsRootsRc::from(self.db.proposals_roots()?))
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

        let pool_reward_accounts = BTreeSet::from([credential(1), credential(2)]);

        assert_eq!(
            unclaimed(&mut view, &pool_reward_accounts),
            BTreeSet::from([credential(2)]),
            "credential(1) has an account and is payable; credential(2) never had one",
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

        assert_eq!(unclaimed(&mut view, &BTreeSet::new()), BTreeSet::from([credential(2)]));
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

        let pool_reward_accounts = BTreeSet::from([credential(1), credential(5)]);

        assert_eq!(unclaimed(&mut view, &pool_reward_accounts), BTreeSet::from([credential(2), credential(4)]));
    }

    // HELPERS

    /// The unclaimed credentials a view yields, gathered into a set so that the order they come in
    /// and any repetition don't matter.
    fn unclaimed(
        view: &mut VolatileView<'_, '_, Stable>,
        pool_reward_accounts: &BTreeSet<StakeCredential>,
    ) -> BTreeSet<StakeCredential> {
        view.unclaimed_reward_accounts(pool_reward_accounts).unwrap().collect()
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
