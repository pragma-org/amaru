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

use amaru_kernel::{Epoch, Point, Pots, ProposalsRoots};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::State,
    store::{
        EpochTransitionProgress, ReadStore, Result, Store,
        columns::{pools, proposals, recently_unregistered_accounts},
    },
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDBTransactionalContext, RocksDbConfig};

pub struct BenchStore {
    pub cfg: RocksDbConfig,
    pub db: RocksDB,
    // Kept alive so the directory outlives the RocksDB connection; dropped last.
    _tempdir: tempfile::TempDir,
}

impl BenchStore {
    #[allow(clippy::expect_used)]
    pub fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("tempdir creation succeeds");
        let cfg = RocksDbConfig::new(tempdir.path().to_path_buf());
        let db = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");
        Self { cfg, db, _tempdir: tempdir }
    }
}

impl ReadStore for BenchStore {
    fn tip(&self) -> Result<Point> {
        self.db.tip()
    }

    fn proposals_roots(&self) -> Result<ProposalsRoots> {
        Ok(ProposalsRoots::default())
    }

    fn pots(&self) -> Result<Pots> {
        Ok(Pots::default())
    }

    fn epoch_transition_progress(&self) -> Result<Option<EpochTransitionProgress>> {
        self.db.epoch_transition_progress()
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

impl Store for BenchStore {
    type Transaction<'a> = RocksDBTransactionalContext<'a>;

    fn next_snapshot(&self, epoch: Epoch) -> Result<()> {
        self.db.next_snapshot(epoch)
    }

    fn create_transaction(&self) -> RocksDBTransactionalContext<'_> {
        self.db.create_transaction()
    }
}

#[expect(clippy::wildcard_enum_match_arm)]
#[expect(clippy::panic)]
pub fn roll_forward(state: &mut State<BenchStore, RocksDBHistoricalStores>, block: &amaru_kernel::Block) {
    use amaru_ledger::rules::block::BlockValidation;
    match state.roll_forward(block, &ArenaPool::new(1024, 0)) {
        BlockValidation::Valid(_) => (),
        other => panic!("block was not applied: {other:?}"),
    }
}
