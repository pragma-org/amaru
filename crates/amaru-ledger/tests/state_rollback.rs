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

use std::collections::VecDeque;

use amaru_kernel::{
    BlockHeight, Epoch, EraHistory, GlobalParameters, Hash, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, ProtocolParameters, Slot, Tip,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::{BackwardError, State, volatile::VolatileFragment},
    store::{EpochTransitionProgress, ReadStore, Store, StoreError},
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

#[test]
fn rollback_to_a_volatile_common_ancestor_succeeds() {
    let mut state = make_state();
    let earlier = point(100, 1);
    let later = point(200, 2);

    assert_eq!(*state.tip(), Point::Origin);

    forward_to(&mut state, earlier, 1);
    forward_to(&mut state, later, 2);
    assert_eq!(*state.tip(), later);

    state.rollback_to(&later).unwrap();
    assert_eq!(*state.tip(), later);

    state.rollback_to(&earlier).unwrap();
    assert_eq!(*state.tip(), earlier);

    state.rollback_to(&Point::Origin).unwrap();
    assert_eq!(*state.tip(), Point::Origin);
}

#[test]
fn rollback_before_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(50, 9);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::UnknownRollbackPoint { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_hash_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(100, 2);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::UnknownRollbackPoint { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_slot_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(150, 1);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::UnknownRollbackPoint { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_after_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);

    let to = point(101, 2);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::RollbackPointInFuture { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(100, 1), "tip is unchanged after a rejected rollback");
}

// HELPERS

/// Create an initial ledger state
#[expect(clippy::expect_used)]
fn make_state() -> State<MockStore, RocksDBHistoricalStores> {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let global_parameters: GlobalParameters = PREPROD_GLOBAL_PARAMETERS.clone();
    let protocol_parameters: ProtocolParameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();

    let ledger_dir = tempfile::tempdir().expect("tempdir creation succeeds").keep();
    let cfg = RocksDbConfig::new(ledger_dir);
    let store = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");
    let snapshots = RocksDBHistoricalStores::new(&cfg, 0);

    State::new_with(
        MockStore(store),
        snapshots,
        Epoch::default(),
        network,
        era_history,
        global_parameters,
        protocol_parameters,
        GovernanceActivity::default(),
        VecDeque::new(),
    )
}

/// Forward the ldeger to a given point
#[expect(clippy::expect_used)]
fn forward_to(state: &mut State<MockStore, RocksDBHistoricalStores>, point: Point, height: u64) {
    let issuer = Hash::new([0u8; 28]);
    let tip = Tip::new(point, BlockHeight::from(height));
    state.push_fragment(VolatileFragment::default().anchor(tip, issuer)).expect("forward");
}

fn point(slot: u64, tag: u8) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([tag; 32]))
}

struct MockStore(RocksDB);

#[expect(unused_variables)]
impl ReadStore for MockStore {
    fn tip(&self) -> amaru_ledger::store::Result<Point> {
        Ok(Point::Origin)
    }

    fn epoch_transition_progress(&self) -> amaru_ledger::store::Result<Option<EpochTransitionProgress>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn protocol_parameters(&self) -> amaru_ledger::store::Result<ProtocolParameters> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn pool(
        &self,
        pool: &amaru_kernel::PoolId,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::pools::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn account(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::accounts::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn drep(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::dreps::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn cc_member(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::cc_members::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn proposal(
        &self,
        id: &amaru_kernel::ComparableProposalId,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::proposals::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn utxo(
        &self,
        input: &amaru_kernel::TransactionInput,
    ) -> amaru_ledger::store::Result<Option<amaru_kernel::MemoizedTransactionOutput>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn pots(&self) -> amaru_ledger::store::Result<amaru_ledger::store::columns::pots::Row> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn constitutional_committee(&self) -> amaru_ledger::store::Result<amaru_kernel::ConstitutionalCommitteeStatus> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn constitution(&self) -> amaru_ledger::store::Result<amaru_kernel::Constitution> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn proposals_roots(&self) -> amaru_ledger::store::Result<amaru_ledger::governance::ratification::ProposalsRoots> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn governance_activity(&self) -> amaru_ledger::store::Result<GovernanceActivity> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_utxos(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::utxo::Key, amaru_ledger::store::columns::utxo::Value)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_block_issuers(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::slots::Key, amaru_ledger::store::columns::slots::Value)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_pools(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::pools::Key, amaru_ledger::store::columns::pools::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_accounts(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::accounts::Key, amaru_ledger::store::columns::accounts::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_dreps(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::dreps::Key, amaru_ledger::store::columns::dreps::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_proposals(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::proposals::Key, amaru_ledger::store::columns::proposals::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_cc_members(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::cc_members::Key, amaru_ledger::store::columns::cc_members::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_votes(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::votes::Key, amaru_ledger::store::columns::votes::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }
}

impl Store for MockStore {
    type Transaction<'a> = <RocksDB as Store>::Transaction<'a>;

    fn next_snapshot(&self, epoch: amaru_kernel::Epoch) -> amaru_ledger::store::Result<()> {
        self.0.next_snapshot(epoch)
    }

    fn create_transaction(&self) -> Self::Transaction<'_> {
        self.0.create_transaction()
    }
}
