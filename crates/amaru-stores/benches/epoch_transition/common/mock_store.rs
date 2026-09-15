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

use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    sync::{Arc, Mutex},
};

use amaru_kernel::{
    Anchor, Constitution, ConstitutionalCommitteeStatus, Credential, Epoch, EraHistory, MaxString128,
    Point, Pots, ProposalId, ProposalsRoots, ProtocolParameters, RatificationStatus,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::State,
    store::{
        Columns, EpochTransitionProgress, ReadStore, Result, Store,
        TransactionalContext,
        columns::{accounts, cc_members, dreps, pools, pots, proposals, recently_unregistered_accounts, slots, utxo, votes},
    },
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

pub struct MockStore {
    pub cfg: RocksDbConfig,
    pub db: RocksDB,
    pub stable: Arc<Mutex<Vec<Point>>>,
    progress: Arc<Mutex<Option<EpochTransitionProgress>>>,
    // Kept alive so the directory outlives the RocksDB connection; dropped last.
    _tempdir: tempfile::TempDir,
}

impl MockStore {
    #[allow(clippy::expect_used)]
    pub fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("tempdir creation succeeds");
        let cfg = RocksDbConfig::new(tempdir.path().to_path_buf());
        let db = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");
        let stable = Arc::new(Mutex::new(Vec::new()));
        Self { cfg, db, stable, progress: Arc::new(Mutex::new(None)), _tempdir: tempdir }
    }
}

impl ReadStore for MockStore {
    #[allow(clippy::unwrap_used)]
    fn tip(&self) -> Result<Point> {
        Ok(self.stable.lock().unwrap().last().copied().unwrap_or(Point::Origin))
    }

    fn proposals_roots(&self) -> Result<ProposalsRoots> {
        Ok(ProposalsRoots::default())
    }

    fn pots(&self) -> Result<Pots> {
        Ok(Pots::default())
    }

    #[allow(clippy::unwrap_used)]
    fn epoch_transition_progress(&self) -> Result<Option<EpochTransitionProgress>> {
        Ok(*self.progress.lock().unwrap())
    }

    fn iter_pools(&self) -> Result<impl Iterator<Item = (pools::Key, pools::Row)>> {
        self.db.iter_pools()
    }

    fn iter_recently_unregistered_accounts(&self) -> Result<impl Iterator<Item = recently_unregistered_accounts::Key>> {
        Ok(std::iter::empty())
    }

    fn iter_proposals(&self) -> Result<impl Iterator<Item = (proposals::Key, proposals::Row)>> {
        Ok(std::iter::empty())
    }

    fn governance_activity(&self) -> Result<GovernanceActivity> {
        Ok(GovernanceActivity::default())
    }
}

impl Store for MockStore {
    type Transaction<'a> = MockTransaction<'a>;

    fn next_snapshot(&self, epoch: Epoch) -> Result<()> {
        self.db.next_snapshot(epoch)
    }

    fn create_transaction(&self) -> MockTransaction<'_> {
        MockTransaction { flushed: &self.stable, progress: &self.progress }
    }
}

pub struct MockTransaction<'a> {
    flushed: &'a Mutex<Vec<Point>>,
    progress: &'a Mutex<Option<EpochTransitionProgress>>,
}

impl ReadStore for MockTransaction<'_> {
    fn governance_activity(&self) -> Result<GovernanceActivity> {
        Ok(GovernanceActivity::default())
    }

    #[allow(clippy::expect_used)]
    fn constitution(&self) -> Result<Constitution> {
        Ok(Constitution {
            anchor: Anchor {
                url: MaxString128::from_str("https://example.com").expect("valid anchor URL"),
                content_hash: [0; 32].into(),
            },
            guardrail_script: None,
        })
    }
}

impl<'a> TransactionalContext<'a> for MockTransaction<'a> {
    fn commit(self) -> Result<()> {
        Ok(())
    }

    #[allow(clippy::unwrap_used)]
    fn reset_epoch_transition_progress(&self) -> Result<()> {
        *self.progress.lock().unwrap() = None;
        Ok(())
    }

    #[allow(clippy::unwrap_used)]
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool> {
        let mut progress = self.progress.lock().unwrap();
        if *progress == from {
            *progress = to;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn set_recently_pruned_proposals<'iter>(
        &self,
        _proposals: impl IntoIterator<Item = (&'iter ProposalId, RatificationStatus)>,
    ) -> Result<()> {
        Ok(())
    }

    fn prune_recently_unregistered_accounts(&self, _epoch: Epoch) -> Result<()> {
        Ok(())
    }

    fn set_proposals_roots(&self, _roots: &ProposalsRoots) -> Result<()> {
        Ok(())
    }

    fn set_protocol_parameters(&self, _protocol_parameters: &ProtocolParameters) -> Result<()> {
        Ok(())
    }

    fn set_governance_activity(&self, _dormant_epochs: GovernanceActivity) -> Result<()> {
        Ok(())
    }

    fn with_accounts(&self, _with: impl FnMut(accounts::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_pools(&self, _with: impl FnMut(pools::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_utxo(&self, _with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_dreps(&self, _with: impl FnMut(dreps::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_cc_members(&self, _with: impl FnMut(cc_members::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn remove_proposals<T>(&self, _proposals: &BTreeMap<ProposalId, T>) -> Result<()> {
        Ok(())
    }

    fn set_constitution(&self, _constitution: &Constitution) -> Result<()> {
        Ok(())
    }

    fn update_constitutional_committee(
        &self,
        _status: &ConstitutionalCommitteeStatus,
        _added: &BTreeMap<Credential, Epoch>,
        _removed: &BTreeSet<Credential>,
    ) -> Result<()> {
        Ok(())
    }

    fn with_block_issuers(&self, _with: impl FnMut(slots::Iter<'_, '_>)) -> Result<()> {
        Ok(())
    }

    fn with_pots(&self, _with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>)) -> Result<()> {
        Ok(())
    }

    #[allow(clippy::unwrap_used)]
    fn save(
        &self,
        _era_history: &EraHistory,
        _protocol_parameters: &ProtocolParameters,
        _governance_activity: Option<GovernanceActivity>,
        point: &Point,
        _issuer: Option<&pools::Key>,
        _add: Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
            impl Iterator<Item = (dreps::Key, dreps::Value)>,
            impl Iterator<Item = (cc_members::Key, cc_members::Value)>,
            impl Iterator<Item = (proposals::Key, proposals::Value)>,
            impl Iterator<Item = (votes::Key, votes::Value)>,
        >,
        _remove: Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
            impl Iterator<Item = (dreps::Key, amaru_kernel::CertificatePointer)>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        _withdrawals: impl Iterator<Item = accounts::Key>,
    ) -> Result<()> {
        self.flushed.lock().unwrap().push(*point);
        Ok(())
    }
}

#[allow(clippy::expect_used)]
pub fn roll_forward(state: &mut State<MockStore, RocksDBHistoricalStores>, block: &amaru_kernel::Block) {
    use amaru_ledger::rules::block::BlockValidation;
    match state.roll_forward(block, &ArenaPool::new(1024, 0)) {
        BlockValidation::Valid(_) => (),
        other => panic!("block was not applied: {other:?}"),
    }
}
