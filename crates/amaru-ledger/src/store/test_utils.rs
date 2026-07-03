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

//! Reusable store mocks for tests.
//!
//! [`MockStore`] behaves like an empty store with a configurable tip: lookups
//! return `None`, iterators are empty, pots are zeroed, and no epoch
//! transition is in progress. It also acts as an (empty) [`Snapshot`] with a
//! configurable epoch, and as a no-op [`TransactionalContext`], so it can
//! stand in for the stable store wherever a [`Store`] is expected.
//! [`MockHistoricalStores`] serves empty snapshots for a configurable list of
//! epochs.

use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
};

use amaru_kernel::{
    Anchor, CertificatePointer, ComparableProposalId, Constitution, ConstitutionalCommitteeStatus, Epoch, EraHistory,
    Hash, Lovelace, MemoizedTransactionOutput, Nullable, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, PoolId,
    ProposalId, ProtocolParameters, StakeCredential, TransactionInput,
};

use crate::{
    epoch_transition::GovernanceActivity,
    governance::ratification::ProposalsRoots,
    store::{
        Columns, EpochTransitionProgress, HistoricalStores, ReadStore, Result, Snapshot, Store, TransactionalContext,
        columns as scolumns,
    },
    summary::Pots,
};

/// An empty store with a configurable tip (and epoch, when used as a
/// [`Snapshot`]).
#[derive(Clone)]
pub struct MockStore {
    tip: Point,
    epoch: Epoch,
}

impl MockStore {
    /// An empty store whose `tip` is the given point.
    pub fn new(tip: Point) -> Self {
        Self { tip, epoch: Epoch::from(0_u64) }
    }

    /// Set the epoch reported when used as a [`Snapshot`].
    pub fn at_epoch(mut self, epoch: Epoch) -> Self {
        self.epoch = epoch;
        self
    }
}

impl ReadStore for MockStore {
    fn tip(&self) -> Result<Point> {
        Ok(self.tip)
    }

    fn epoch_transition_progress(&self) -> Result<Option<EpochTransitionProgress>> {
        Ok(None)
    }

    fn protocol_parameters(&self) -> Result<ProtocolParameters> {
        Ok(PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone())
    }

    fn pool(&self, _pool: &PoolId) -> Result<Option<scolumns::pools::Row>> {
        Ok(None)
    }

    fn account(&self, _credential: &StakeCredential) -> Result<Option<scolumns::accounts::Row>> {
        Ok(None)
    }

    fn drep(&self, _credential: &StakeCredential) -> Result<Option<scolumns::dreps::Row>> {
        Ok(None)
    }

    fn cc_member(&self, _credential: &StakeCredential) -> Result<Option<scolumns::cc_members::Row>> {
        Ok(None)
    }

    fn proposal(&self, _id: &ComparableProposalId) -> Result<Option<scolumns::proposals::Row>> {
        Ok(None)
    }

    fn utxo(&self, _input: &TransactionInput) -> Result<Option<MemoizedTransactionOutput>> {
        Ok(None)
    }

    fn pots(&self) -> Result<Pots> {
        Ok(Pots { treasury: 0, reserves: 0, fees: 0 })
    }

    fn constitutional_committee(&self) -> Result<ConstitutionalCommitteeStatus> {
        Ok(ConstitutionalCommitteeStatus::NoConfidence)
    }

    fn constitution(&self) -> Result<Constitution> {
        Ok(Constitution {
            anchor: Anchor { url: String::new(), content_hash: Hash::from([0u8; 32]) },
            guardrail_script: Nullable::Null,
        })
    }

    fn proposals_roots(&self) -> Result<ProposalsRoots> {
        Ok(ProposalsRoots::default())
    }

    fn governance_activity(&self) -> Result<GovernanceActivity> {
        Ok(GovernanceActivity::default())
    }

    fn iter_utxos(&self) -> Result<impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>> {
        Ok(std::iter::empty())
    }

    fn iter_block_issuers(&self) -> Result<impl Iterator<Item = (scolumns::slots::Key, scolumns::slots::Value)>> {
        Ok(std::iter::empty())
    }

    fn iter_pools(&self) -> Result<impl Iterator<Item = (scolumns::pools::Key, scolumns::pools::Row)>> {
        Ok(std::iter::empty())
    }

    fn iter_accounts(&self) -> Result<impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Row)>> {
        Ok(std::iter::empty())
    }

    fn iter_dreps(&self) -> Result<impl Iterator<Item = (scolumns::dreps::Key, scolumns::dreps::Row)>> {
        Ok(std::iter::empty())
    }

    fn iter_proposals(&self) -> Result<impl Iterator<Item = (scolumns::proposals::Key, scolumns::proposals::Row)>> {
        Ok(std::iter::empty())
    }

    fn iter_cc_members(&self) -> Result<impl Iterator<Item = (scolumns::cc_members::Key, scolumns::cc_members::Row)>> {
        Ok(std::iter::empty())
    }

    fn iter_votes(&self) -> Result<impl Iterator<Item = (scolumns::votes::Key, scolumns::votes::Row)>> {
        Ok(std::iter::empty())
    }
}

impl Snapshot for MockStore {
    fn epoch(&self) -> Epoch {
        self.epoch
    }
}

impl<'a> TransactionalContext<'a> for MockStore {
    fn commit(self) -> Result<()> {
        Ok(())
    }

    fn rollback(self) -> Result<()> {
        Ok(())
    }

    fn reset_epoch_transition_progress(&self) -> Result<()> {
        Ok(())
    }

    fn try_epoch_transition(
        &self,
        _from: Option<EpochTransitionProgress>,
        _to: Option<EpochTransitionProgress>,
    ) -> Result<bool> {
        Ok(true)
    }

    fn save(
        &self,
        _era_history: &EraHistory,
        _protocol_parameters: &ProtocolParameters,
        _governance_activity: GovernanceActivity,
        _point: &Point,
        _issuer: Option<&scolumns::pools::Key>,
        _add: Columns<
            impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>,
            impl Iterator<Item = scolumns::pools::Value>,
            impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Value)>,
            impl Iterator<Item = (scolumns::dreps::Key, scolumns::dreps::Value)>,
            impl Iterator<Item = (scolumns::cc_members::Key, scolumns::cc_members::Value)>,
            impl Iterator<Item = (scolumns::proposals::Key, scolumns::proposals::Value)>,
            impl Iterator<Item = (scolumns::votes::Key, scolumns::votes::Value)>,
        >,
        _remove: Columns<
            impl Iterator<Item = scolumns::utxo::Key>,
            impl Iterator<Item = (scolumns::pools::Key, Epoch)>,
            impl Iterator<Item = scolumns::accounts::Key>,
            impl Iterator<Item = (scolumns::dreps::Key, CertificatePointer)>,
            impl Iterator<Item = scolumns::cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        _withdrawals: impl Iterator<Item = scolumns::accounts::Key>,
    ) -> Result<()> {
        Ok(())
    }

    fn refund(&self, _credential: &scolumns::accounts::Key, _deposit: Lovelace) -> Result<Lovelace> {
        Ok(0)
    }

    fn set_protocol_parameters(&self, _protocol_parameters: &ProtocolParameters) -> Result<()> {
        Ok(())
    }

    fn update_constitutional_committee(
        &self,
        _status: &ConstitutionalCommitteeStatus,
        _added: BTreeMap<StakeCredential, Epoch>,
        _removed: BTreeSet<StakeCredential>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_proposals_roots(&self, _roots: &ProposalsRoots) -> Result<()> {
        Ok(())
    }

    fn set_constitution(&self, _constitution: &Constitution) -> Result<()> {
        Ok(())
    }

    fn set_governance_activity(&self, _dormant_epochs: GovernanceActivity) -> Result<()> {
        Ok(())
    }

    fn remove_proposals<'iter, Id>(&self, _proposals: impl IntoIterator<Item = Id>) -> Result<()>
    where
        Id: Deref<Target = ProposalId> + 'iter,
    {
        Ok(())
    }

    fn with_pots(&self, _with: impl FnMut(Box<dyn BorrowMut<scolumns::pots::Row> + '_>)) -> Result<()> {
        Ok(())
    }

    fn with_pools(&self, _with: impl FnMut(scolumns::pools::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_accounts(&self, _with: impl FnMut(scolumns::accounts::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_block_issuers(&self, _with: impl FnMut(scolumns::slots::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_utxo(&self, _with: impl FnMut(scolumns::utxo::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_dreps(&self, _with: impl FnMut(scolumns::dreps::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_proposals(&self, _with: impl FnMut(scolumns::proposals::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_cc_members(&self, _with: impl FnMut(scolumns::cc_members::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }
}

impl Store for MockStore {
    type Transaction<'a> = MockStore;

    fn next_snapshot(&self, _epoch: Epoch) -> Result<()> {
        Ok(())
    }

    fn create_transaction(&self) -> Self::Transaction<'_> {
        self.clone()
    }
}

/// Empty snapshots for a configurable list of epochs.
pub struct MockHistoricalStores {
    snapshots: Vec<Epoch>,
}

impl MockHistoricalStores {
    /// Serve (empty) snapshots for the given epochs, oldest first.
    pub fn new(snapshots: Vec<Epoch>) -> Self {
        Self { snapshots }
    }
}

impl HistoricalStores for MockHistoricalStores {
    fn snapshots(&self) -> Result<Vec<Epoch>> {
        Ok(self.snapshots.clone())
    }

    fn prune(&self, _minimum_epoch: Epoch) -> Result<()> {
        Ok(())
    }

    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot> {
        Ok(MockStore::new(Point::Origin).at_epoch(epoch))
    }
}
