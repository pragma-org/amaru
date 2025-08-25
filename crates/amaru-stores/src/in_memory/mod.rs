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

use crate::in_memory::ledger::columns::{
    accounts, cc_members, dreps, pools, proposals, utxo, votes,
};
use amaru_kernel::{
    network::NetworkName, protocol_parameters::ProtocolParameters, ComparableProposalId,
    Constitution, ConstitutionalCommittee, EraHistory, Lovelace, Point, PoolId, Slot,
    StakeCredential, TransactionInput,
};
use amaru_ledger::{
    governance::ratification::{ProposalsRoots, ProposalsRootsRc},
    store::{
        columns::{
            accounts as accounts_column, cc_members as cc_members_column, dreps as dreps_column,
            pools as pools_column, pots, proposals as proposals_column, slots, utxo as utxo_column,
            votes as votes_column,
        },
        EpochTransitionProgress, GovernanceActivity, HistoricalStores, ReadStore, Snapshot, Store,
        StoreError, TransactionalContext,
    },
};
use iter_borrow::IterBorrow;
use progress_bar::ProgressBar;
use slot_arithmetic::Epoch;
use std::{
    borrow::{Borrow, BorrowMut},
    cell::{RefCell, RefMut},
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};
pub mod ledger;
mod memory_snapshot;
use memory_snapshot::MemorySnapshot;

#[derive(Clone)]
pub struct MemoryStore {
    tip: RefCell<Point>,
    epoch_progress: RefCell<Option<EpochTransitionProgress>>,
    constitution: RefCell<Option<Constitution>>,
    proposals_roots: RefCell<Option<ProposalsRoots>>,
    governance_activity: RefCell<Option<GovernanceActivity>>,
    dreps: RefCell<BTreeMap<StakeCredential, dreps_column::Row>>,
    proposals: RefCell<BTreeMap<ComparableProposalId, proposals_column::Row>>,
    cc_members: RefCell<BTreeMap<StakeCredential, cc_members_column::Row>>,
    votes: RefCell<BTreeMap<votes_column::Key, votes_column::Value>>,
    protocol_parameters: RefCell<Option<ProtocolParameters>>,
    constitutional_committee: RefCell<ConstitutionalCommittee>,
    era_history: EraHistory,
    utxos: RefCell<BTreeMap<TransactionInput, utxo_column::Value>>,
    accounts: RefCell<BTreeMap<StakeCredential, accounts_column::Row>>,
    pools: RefCell<BTreeMap<PoolId, pools_column::Row>>,
    pots: RefCell<pots::Row>,
    slots: RefCell<BTreeMap<Slot, slots::Row>>,

    // TODO: optimize snapshot storage using OrdMap to avoid duplicating unchanged entries
    snapshots: RefCell<BTreeMap<Epoch, MemorySnapshot>>,
}

impl MemoryStore {
    pub fn new(era_history: EraHistory) -> Self {
        MemoryStore {
            tip: RefCell::new(Point::Origin),
            epoch_progress: RefCell::new(None),
            constitution: RefCell::new(None),
            proposals_roots: RefCell::new(None),
            governance_activity: RefCell::new(None),
            utxos: RefCell::new(BTreeMap::new()),
            accounts: RefCell::new(BTreeMap::new()),
            pools: RefCell::new(BTreeMap::new()),
            pots: RefCell::new(pots::Row::default()),
            slots: RefCell::new(BTreeMap::new()),
            dreps: RefCell::new(BTreeMap::new()),
            proposals: RefCell::new(BTreeMap::new()),
            cc_members: RefCell::new(BTreeMap::new()),
            votes: RefCell::new(BTreeMap::new()),
            protocol_parameters: RefCell::new(None),
            constitutional_committee: RefCell::new(ConstitutionalCommittee::NoConfidence),
            era_history,
            snapshots: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn apply_snapshot_bytes(
        &mut self,
        bytes: &[u8],
        point: &amaru_kernel::Point,
        network: NetworkName,
        with_progress: &dyn Fn(usize, &str) -> Box<dyn ProgressBar>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        amaru_ledger::bootstrap::import_initial_snapshot(
            self,
            bytes,
            point,
            network,
            with_progress,
            None,
            true,
        )?;

        let epoch = self.epoch();
        tracing::info!(
            ?epoch,
            utxo_count = self.utxos.borrow().len(),
            account_count = self.accounts.borrow().len(),
            pool_count = self.pools.borrow().len(),
            drep_count = self.dreps.borrow().len(),
            "Applied snapshot"
        );
        Ok(())
    }
}

impl Snapshot for MemoryStore {
    fn epoch(&self) -> Epoch {
        let point = self.tip.borrow();
        let slot = point.slot_or_default();
        self.era_history
            .slot_to_epoch(slot, slot)
            .unwrap_or_else(|_| Epoch::from(0))
    }
}

impl ReadStore for MemoryStore {
    fn tip(&self) -> Result<Point, StoreError> {
        Ok(self.tip.borrow().clone())
    }

    fn protocol_parameters(&self) -> Result<ProtocolParameters, StoreError> {
        self.protocol_parameters
            .borrow()
            .clone()
            .ok_or(StoreError::missing::<ProtocolParameters>(
                "protocol-parameters",
            ))
    }

    /// Get the latest governance roots; which corresponds to the id of the latest governance
    /// actions enacted for specific categories.
    fn proposals_roots(&self) -> Result<ProposalsRoots, StoreError> {
        self.proposals_roots
            .borrow()
            .clone()
            .ok_or_else(|| StoreError::missing::<ProposalsRoots>("proposals_roots"))
    }

    fn constitutional_committee(&self) -> Result<ConstitutionalCommittee, StoreError> {
        Ok(self.constitutional_committee.borrow().clone())
    }

    fn constitution(&self) -> Result<Constitution, StoreError> {
        self.constitution
            .borrow()
            .clone()
            .ok_or(StoreError::missing::<Constitution>("constitution"))
    }

    fn governance_activity(&self) -> Result<GovernanceActivity, StoreError> {
        self.governance_activity
            .borrow()
            .clone()
            .ok_or(StoreError::missing::<GovernanceActivity>(
                "governance_activity",
            ))
    }

    fn account(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> Result<Option<amaru_ledger::store::columns::accounts::Row>, amaru_ledger::store::StoreError>
    {
        Ok(self.accounts.borrow().get(credential).cloned())
    }

    fn pool(
        &self,
        pool: &amaru_kernel::PoolId,
    ) -> Result<Option<amaru_ledger::store::columns::pools::Row>, amaru_ledger::store::StoreError>
    {
        Ok(self.pools.borrow().get(pool).cloned())
    }

    fn utxo(
        &self,
        input: &amaru_kernel::TransactionInput,
    ) -> Result<Option<amaru_kernel::MemoizedTransactionOutput>, amaru_ledger::store::StoreError>
    {
        Ok(self.utxos.borrow().get(input).cloned())
    }

    fn pots(&self) -> Result<amaru_ledger::summary::Pots, amaru_ledger::store::StoreError> {
        Ok((&*self.pots.borrow()).into())
    }

    #[allow(refining_impl_trait)]
    fn iter_utxos(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::utxo::Key,
                amaru_ledger::store::columns::utxo::Value,
            ),
        >,
        amaru_ledger::store::StoreError,
    > {
        let utxo_vec: Vec<_> = self
            .utxos
            .borrow()
            .iter()
            .map(|(tx_input, tx_output)| (tx_input.clone(), tx_output.clone()))
            .collect();
        Ok(utxo_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_block_issuers(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::slots::Key,
                amaru_ledger::store::columns::slots::Value,
            ),
        >,
        StoreError,
    > {
        let block_issuer_vec: Vec<_> = self
            .slots
            .borrow()
            .iter()
            .map(|(slot, row)| (*slot, row.clone()))
            .collect();

        Ok(block_issuer_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_pools(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::pools::Key,
                amaru_ledger::store::columns::pools::Row,
            ),
        >,
        StoreError,
    > {
        let pool_vec: Vec<_> = self
            .pools
            .borrow()
            .iter()
            .map(|(pool_id, row)| (*pool_id, row.clone()))
            .collect();

        Ok(pool_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_accounts(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::accounts::Key,
                amaru_ledger::store::columns::accounts::Row,
            ),
        >,
        StoreError,
    > {
        let accounts_vec: Vec<_> = self
            .accounts
            .borrow()
            .iter()
            .map(|(stake_credential, row)| (stake_credential.clone(), row.clone()))
            .collect();

        Ok(accounts_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_dreps(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::dreps::Key,
                amaru_ledger::store::columns::dreps::Row,
            ),
        >,
        StoreError,
    > {
        let dreps_vec: Vec<_> = self
            .dreps
            .borrow()
            .iter()
            .map(|(stake_credential, row)| (stake_credential.clone(), row.clone()))
            .collect();

        Ok(dreps_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_proposals(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::proposals::Key,
                amaru_ledger::store::columns::proposals::Row,
            ),
        >,
        StoreError,
    > {
        let proposals_vec: Vec<_> = self
            .proposals
            .borrow()
            .iter()
            .map(|(proposal_id, row)| (proposal_id.clone(), row.clone()))
            .collect();

        Ok(proposals_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_cc_members(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::cc_members::Key,
                amaru_ledger::store::columns::cc_members::Row,
            ),
        >,
        StoreError,
    > {
        let cc_members_vec: Vec<_> = self
            .cc_members
            .borrow()
            .iter()
            .map(|(key, row)| (key.clone(), row.clone()))
            .collect();

        Ok(cc_members_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_votes(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::votes::Key,
                amaru_ledger::store::columns::votes::Row,
            ),
        >,
        StoreError,
    > {
        let votes_vec: Vec<_> = self
            .votes
            .borrow()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        Ok(votes_vec.into_iter())
    }
}

pub struct MemoryTransactionalContext<'a> {
    store: &'a MemoryStore,
}

impl<'a> MemoryTransactionalContext<'a> {
    pub fn new(store: &'a MemoryStore) -> Self {
        Self { store }
    }
}

impl<'a> MemoryTransactionalContext<'a> {
    pub fn with_column<K, V, M, F>(
        &self,
        column: &RefCell<M>,
        mut with: F,
    ) -> Result<(), StoreError>
    where
        K: Clone + Ord + 'a,
        V: Clone + 'a,
        M: IntoIterator<Item = (K, V)> + FromIterator<(K, V)> + Default,
        F: for<'iter> FnMut(IterBorrow<'iter, 'iter, K, Option<V>>),
    {
        let original: M = column.take();
        let mut values: Vec<(K, Option<V>)> =
            original.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = k.clone();
            let boxed: Box<dyn BorrowMut<Option<V>>> = Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let rebuilt: M = values
            .into_iter()
            .flat_map(|(k, v)| v.map(|v| (k, v)))
            .collect();

        column.replace(rebuilt);
        Ok(())
    }
}

impl<'a> ReadStore for MemoryTransactionalContext<'a> {
    fn tip(&self) -> Result<Point, StoreError> {
        self.store.tip()
    }

    fn protocol_parameters(&self) -> Result<ProtocolParameters, StoreError> {
        self.store.protocol_parameters()
    }

    fn proposals_roots(&self) -> Result<ProposalsRoots, StoreError> {
        self.store.proposals_roots()
    }

    fn constitutional_committee(&self) -> Result<ConstitutionalCommittee, StoreError> {
        self.store.constitutional_committee()
    }

    fn constitution(&self) -> Result<Constitution, StoreError> {
        self.store.constitution()
    }

    fn governance_activity(&self) -> Result<GovernanceActivity, StoreError> {
        self.store.governance_activity()
    }

    fn account(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> Result<Option<amaru_ledger::store::columns::accounts::Row>, amaru_ledger::store::StoreError>
    {
        self.store.account(credential)
    }

    fn pool(
        &self,
        pool: &amaru_kernel::PoolId,
    ) -> Result<Option<amaru_ledger::store::columns::pools::Row>, amaru_ledger::store::StoreError>
    {
        self.store.pool(pool)
    }

    fn utxo(
        &self,
        input: &amaru_kernel::TransactionInput,
    ) -> Result<Option<amaru_kernel::MemoizedTransactionOutput>, amaru_ledger::store::StoreError>
    {
        self.store.utxo(input)
    }

    fn pots(&self) -> Result<amaru_ledger::summary::Pots, amaru_ledger::store::StoreError> {
        self.store.pots()
    }

    #[allow(refining_impl_trait)]
    fn iter_utxos(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::utxo::Key,
                amaru_ledger::store::columns::utxo::Value,
            ),
        >,
        amaru_ledger::store::StoreError,
    > {
        self.store.iter_utxos()
    }

    #[allow(refining_impl_trait)]
    fn iter_block_issuers(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::slots::Key,
                amaru_ledger::store::columns::slots::Value,
            ),
        >,
        StoreError,
    > {
        self.store.iter_block_issuers()
    }

    #[allow(refining_impl_trait)]
    fn iter_pools(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::pools::Key,
                amaru_ledger::store::columns::pools::Row,
            ),
        >,
        StoreError,
    > {
        self.store.iter_pools()
    }

    #[allow(refining_impl_trait)]
    fn iter_accounts(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::accounts::Key,
                amaru_ledger::store::columns::accounts::Row,
            ),
        >,
        StoreError,
    > {
        self.store.iter_accounts()
    }

    #[allow(refining_impl_trait)]
    fn iter_dreps(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::dreps::Key,
                amaru_ledger::store::columns::dreps::Row,
            ),
        >,
        StoreError,
    > {
        self.store.iter_dreps()
    }

    #[allow(refining_impl_trait)]
    fn iter_proposals(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::proposals::Key,
                amaru_ledger::store::columns::proposals::Row,
            ),
        >,
        StoreError,
    > {
        self.store.iter_proposals()
    }

    #[allow(refining_impl_trait)]
    fn iter_cc_members(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::cc_members::Key,
                amaru_ledger::store::columns::cc_members::Row,
            ),
        >,
        StoreError,
    > {
        self.store.iter_cc_members()
    }

    #[allow(refining_impl_trait)]
    fn iter_votes(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::votes::Key,
                amaru_ledger::store::columns::votes::Row,
            ),
        >,
        StoreError,
    > {
        self.store.iter_votes()
    }
}

impl<'a> TransactionalContext<'a> for MemoryTransactionalContext<'a> {
    fn commit(self) -> Result<(), StoreError> {
        Ok(())
    }

    fn rollback(self) -> Result<(), StoreError> {
        Ok(())
    }

    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool, StoreError> {
        let mut progress = self.store.epoch_progress.borrow_mut();

        if *progress != from {
            return Ok(false);
        }
        *progress = to;
        Ok(true)
    }

    fn refund(
        &self,
        credential: &amaru_ledger::store::columns::accounts::Key,
        deposit: Lovelace,
    ) -> Result<Lovelace, StoreError> {
        let mut accounts = self.store.accounts.borrow_mut();
        match accounts.get_mut(credential) {
            Some(account) => {
                account.rewards += deposit;
                Ok(0)
            }
            _ => Ok(deposit),
        }
    }

    fn set_protocol_parameters(
        &self,
        protocol_parameters: &ProtocolParameters,
    ) -> Result<(), StoreError> {
        *self.store.protocol_parameters.borrow_mut() = Some(protocol_parameters.clone());
        Ok(())
    }

    fn set_constitutional_committee(
        &self,
        constitutional_committee: &ConstitutionalCommittee,
    ) -> Result<(), StoreError> {
        *self.store.constitutional_committee.borrow_mut() = constitutional_committee.clone();
        Ok(())
    }

    fn set_proposals_roots(&self, roots: &ProposalsRootsRc) -> Result<(), StoreError> {
        let plain = ProposalsRoots {
            protocol_parameters: roots.protocol_parameters.as_ref().map(|id| (**id).clone()),
            hard_fork: roots.hard_fork.as_ref().map(|id| (**id).clone()),
            constitutional_committee: roots
                .constitutional_committee
                .as_ref()
                .map(|id| (**id).clone()),
            constitution: roots.constitution.as_ref().map(|id| (**id).clone()),
        };
        *self.store.proposals_roots.borrow_mut() = Some(plain);
        Ok(())
    }

    fn set_constitution(&self, constitution: &Constitution) -> Result<(), StoreError> {
        *self.store.constitution.borrow_mut() = Some(constitution.clone());
        Ok(())
    }

    fn set_governance_activity(
        &self,
        governance_activity: &GovernanceActivity,
    ) -> Result<(), StoreError> {
        *self.store.governance_activity.borrow_mut() = Some(governance_activity.clone());
        Ok(())
    }

    fn remove_proposals<'iter, Id>(
        &self,
        proposals: impl IntoIterator<Item = Id>,
    ) -> Result<(), StoreError>
    where
        Id: Deref<Target = ComparableProposalId> + 'iter,
    {
        let mut proposals_map = self.store.proposals.borrow_mut();

        for id in proposals {
            proposals_map.remove(id.deref());
        }

        Ok(())
    }

    fn save(
        &self,
        _era_history: &EraHistory,
        _protocol_parameters: &ProtocolParameters,
        _governance_activity: &mut GovernanceActivity,
        point: &Point,
        issuer: Option<&amaru_ledger::store::columns::pools::Key>,
        add: amaru_ledger::store::Columns<
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::utxo::Key,
                    amaru_ledger::store::columns::utxo::Value,
                ),
            >,
            impl Iterator<Item = amaru_ledger::store::columns::pools::Value>,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::accounts::Key,
                    amaru_ledger::store::columns::accounts::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::dreps::Key,
                    amaru_ledger::store::columns::dreps::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::cc_members::Key,
                    amaru_ledger::store::columns::cc_members::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::proposals::Key,
                    amaru_ledger::store::columns::proposals::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::votes::Key,
                    amaru_ledger::store::columns::votes::Value,
                ),
            >,
        >,
        remove: amaru_ledger::store::Columns<
            impl Iterator<Item = amaru_ledger::store::columns::utxo::Key>,
            impl Iterator<Item = (amaru_ledger::store::columns::pools::Key, Epoch)>,
            impl Iterator<Item = amaru_ledger::store::columns::accounts::Key>,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::dreps::Key,
                    amaru_kernel::CertificatePointer,
                ),
            >,
            impl Iterator<Item = amaru_ledger::store::columns::cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        withdrawals: impl Iterator<Item = amaru_ledger::store::columns::accounts::Key>,
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let current_tip = self.store.tip.borrow().clone();

        match (point, current_tip) {
            (Point::Specific(new, _), Point::Specific(current, _)) if *new < current => {
                tracing::trace!(target: "event.store.memory", ?point, "save.point_already_known");
                return Ok(());
            }
            _ => {
                // Update tip
                *self.store.tip.borrow_mut() = point.clone();

                // Add issuer to new slot row
                if let Point::Specific(slot, _) = point {
                    if let Some(issuer) = issuer {
                        self.store.slots.borrow_mut().insert(
                            Slot::from(*slot),
                            amaru_ledger::store::columns::slots::Row::new(*issuer),
                        );
                    }
                }
            }
        }

        utxo::add(self.store, add.utxo)?;
        pools::add(self.store, add.pools)?;
        dreps::add(self.store, add.dreps)?;
        accounts::add(self.store, add.accounts)?;
        cc_members::add(self.store, add.cc_members)?;
        proposals::add(self.store, add.proposals)?;
        votes::add(self.store, add.votes)?;

        accounts::reset_many(self.store, withdrawals)?;

        utxo::remove(self.store, remove.utxo)?;
        pools::remove(self.store, remove.pools)?;
        accounts::remove(self.store, remove.accounts)?;
        dreps::remove(self.store, remove.dreps)?;
        cc_members::remove(self.store, remove.cc_members)?;

        Ok(())
    }

    fn clear_pools(&self) -> Result<(), StoreError> {
        Ok(())
    }

    fn clear_accounts(&self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn clear_utxos(&self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn clear_cc_members(&self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn clear_dreps(&self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn clear_proposals(&self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn clear_block_issuers(&self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_pots(
        &self,
        mut with: impl FnMut(Box<dyn BorrowMut<pots::Row> + 'a>),
    ) -> Result<(), StoreError> {
        with(Box::new(RefMutAdapter::new(self.store.pots.borrow_mut())));

        Ok(())
    }

    fn with_pools(&self, with: impl FnMut(pools_column::Iter<'_, '_>)) -> Result<(), StoreError> {
        self.with_column(&self.store.pools, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(accounts_column::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.accounts, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::slots::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.slots, with)
    }

    fn with_utxo(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::utxo::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.utxos, with)
    }

    fn with_dreps(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::dreps::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.dreps, with)
    }

    fn with_proposals(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::proposals::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        self.with_column(&self.store.proposals, with)
    }

    fn with_cc_members(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::cc_members::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        self.with_column(&self.store.cc_members, with)
    }
}

impl Store for MemoryStore {
    type Transaction<'a> = MemoryTransactionalContext<'a>;

    fn next_snapshot(&self, epoch: Epoch) -> Result<(), StoreError> {
        let snapshot = MemorySnapshot::new(epoch, self);
        self.snapshots.borrow_mut().insert(epoch, snapshot);
        Ok(())
    }

    fn create_transaction(&self) -> Self::Transaction<'_> {
        MemoryTransactionalContext { store: self }
    }
}

impl HistoricalStores for MemoryStore {
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError> {
        Ok(self.snapshots.borrow().keys().cloned().collect())
    }

    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot, StoreError> {
        self.snapshots
            .borrow()
            .get(&epoch)
            .cloned()
            .ok_or_else(|| StoreError::missing::<MemorySnapshot>("snapshot"))
    }
}

pub struct RefMutAdapter<'a, T> {
    inner: RefMut<'a, T>,
}

impl<'a, T> RefMutAdapter<'a, T> {
    pub fn new(inner: RefMut<'a, T>) -> Self {
        Self { inner }
    }
}

impl<'a, T> Borrow<T> for RefMutAdapter<'a, T> {
    fn borrow(&self) -> &T {
        self.inner.deref()
    }
}

impl<'a, T> BorrowMut<T> for RefMutAdapter<'a, T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}

pub struct RefMutAdapterMut<'a, T> {
    inner: &'a mut T,
}

impl<'a, T> RefMutAdapterMut<'a, T> {
    pub fn new(inner: &'a mut T) -> Self {
        Self { inner }
    }
}

impl<'a, T> Borrow<T> for RefMutAdapterMut<'a, T> {
    fn borrow(&self) -> &T {
        self.inner
    }
}

impl<'a, T> BorrowMut<T> for RefMutAdapterMut<'a, T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        in_memory::MemoryStore,
        tests::{
            add_test_data_to_store, test_epoch_transition, test_read_account, test_read_drep,
            test_read_pool, test_read_utxo, test_refund_account, test_remove_account,
            test_remove_drep, test_remove_pool, test_remove_utxo, test_slot_updated, Fixture,
        },
    };
    use amaru_kernel::{network::NetworkName, EraHistory};
    use amaru_ledger::store::StoreError;
    use proptest::test_runner::TestRunner;

    #[cfg(not(target_os = "windows"))]
    use crate::tests::test_read_proposal;

    pub fn setup_memory_store(
        runner: &mut TestRunner,
    ) -> Result<(MemoryStore, Fixture), StoreError> {
        let era_history: &EraHistory = NetworkName::Preprod.into();
        let store = MemoryStore::new(era_history.clone());
        let fixture = add_test_data_to_store(&store, era_history, runner)?;
        Ok((store, fixture))
    }

    #[test]
    fn test_in_mem_read_account() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_account(&store, &fixture);
    }

    #[test]
    fn test_in_mem_read_pool() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_pool(&store, &fixture);
    }

    #[test]
    fn test_in_mem_read_drep() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_drep(&store, &fixture);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_in_mem_read_proposal() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_proposal(&store, &fixture);
    }

    #[test]
    fn test_in_mem_refund_account() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_refund_account(&store, &fixture, &mut runner)
    }

    #[test]
    fn test_in_mem_epoch_transition() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, _) = setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_epoch_transition(&store)
    }

    #[test]
    fn test_in_mem_slot_updated() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_slot_updated(&store, &fixture)
    }

    #[test]
    fn test_in_mem_read_utxo() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_utxo(&store, &fixture);
    }

    #[test]
    fn test_in_mem_remove_utxo() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_utxo(&store, &fixture)
    }

    #[test]
    fn test_in_mem_remove_account() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_account(&store, &fixture)
    }

    #[test]
    fn test_in_mem_remove_pool() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_pool(&store, &fixture)
    }

    #[test]
    fn test_in_mem_remove_drep() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_drep(&store, &fixture)
    }

    #[test]
    #[ignore]
    fn test_in_mem_iterate_cc_members() {
        unimplemented!()
    }

    #[test]
    #[ignore]
    fn test_in_mem_remove_cc_members() {
        unimplemented!()
    }
}
