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
    StakeCredential,
};

use crate::{
    governance::ratification::ProposalsRootsRc,
    state::{
        VolatileDB,
        diff_bind::DiffBind,
        diff_epoch_reg::DiffEpochReg,
        volatile::{VolatileSequence, fragment::add_proposals},
    },
    store::{
        ReadStore, StoreError,
        columns::{pools::Row as Pool, *},
    },
};

mod iter_accounts;
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
            accounts.append(anchored.fragment.accounts.as_borrowed());

            pools.append(anchored.fragment.pools.as_borrowed());

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
                    // Delegations only needs not to appear here as they'll be available from the
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
            Some(mut pools) => Ok(iter_pools::IterPools::new(self.epoch, self.db.iter_pools()?, &mut pools)),
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

    /// Provides an iterator for accounts on top of the stable store, also applying any pending
    /// registration or deregistration from the aggregated volatile state.
    ///
    /// IMPORTANT: Yields accounts in no particular order.
    pub fn iter_accounts(&mut self) -> Result<impl Iterator<Item = StakeCredential>, StoreError> {
        match mem::take(&mut self.accounts) {
            None => {
                // Just being careful here. There's no reason to ever call this twice; but if it
                // ever happens, this line might save us from hours of debugging.
                unreachable!(".iter_accounts() called twice on the same VolatileView! Don't do that.")
            }
            Some(mut accounts) => Ok(iter_accounts::IterAccounts::new(
                self.db.iter_accounts()?,
                &mut accounts.registered,
                &mut accounts.unregistered,
            )),
        }
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
