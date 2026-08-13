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

//! Integration tests for ledger `State` rollback against the RocksDB store backend.
//!
//! Lives here (not in `amaru-ledger`) so that `amaru-ledger` need not depend on `amaru-stores`,
//! even as a dev-dependency.

use std::{
    collections::VecDeque,
    fmt::{Debug, Display},
};

use amaru_kernel::{
    Block, BlockHeight, Epoch, EraHistory, GlobalParameters, Hash, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, ProtocolParameters, Slot, Tip,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    rules::block::BlockValidation,
    state::{BackwardError, State, volatile::VolatileFragment},
    store::{HistoricalStores, ReadStore, Store},
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};
use anyhow::anyhow;

fn rollback_to<S, HS>(state: &mut State<S, HS>, point: &Point) -> Result<(), anyhow::Error>
where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
{
    match state.switch_to_fork(point, std::iter::empty(), &ArenaPool::new(1024, 0)) {
        BlockValidation::Valid(..) => Ok(()),
        BlockValidation::Err(err) => Err(err),
        BlockValidation::Invalid(_, _, err) => Err(anyhow!(err)),
    }
}

#[allow(clippy::panic)]
fn assert_no_rollback_to<S, HS, E>(state: &mut State<S, HS>, point: &Point, assert: impl FnOnce(&E))
where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
    E: Display + Debug + Send + Sync + 'static,
{
    assert_no_rollback_to_with_blocks(state, point, std::iter::empty(), assert)
}

#[allow(clippy::panic)]
fn assert_no_rollback_to_with_blocks<I, S, HS, E>(
    state: &mut State<S, HS>,
    point: &Point,
    blocks: I,
    assert: impl FnOnce(&E),
) where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
    E: Display + Debug + Send + Sync + 'static,
    I: IntoIterator<Item = anyhow::Result<(Point, Block)>>,
    I::IntoIter: ExactSizeIterator,
{
    let err = match state.switch_to_fork(point, blocks, &ArenaPool::new(1024, 0)) {
        BlockValidation::Valid(..) => panic!("expected rollback to {point:?} to fail but it was successful"),
        BlockValidation::Err(err) => err,
        BlockValidation::Invalid(_, _, err) => anyhow!(err),
    };

    assert(
        err.downcast_ref::<E>()
            .unwrap_or_else(|| panic!("rollback failed but returned a different error than expected")),
    );
}

#[test]
fn rollback_to_a_volatile_common_ancestor_succeeds() {
    let mut state = make_state();
    let earlier = point(100, 1);
    let later = point(200, 2);

    assert_eq!(*state.tip(), Point::Origin);

    forward_to(&mut state, earlier, 1);
    forward_to(&mut state, later, 2);
    assert_eq!(*state.tip(), later);

    rollback_to(&mut state, &later).unwrap();
    assert_eq!(*state.tip(), later);

    rollback_to(&mut state, &earlier).unwrap();
    assert_eq!(*state.tip(), earlier);

    rollback_to(&mut state, &Point::Origin).unwrap();
    assert_eq!(*state.tip(), Point::Origin);
}

#[test]
fn rollback_before_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(50, 9);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_hash_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(100, 2);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_slot_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(150, 1);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_after_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);

    let to = point(101, 2);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::RollbackPointInFuture { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(100, 1), "tip is unchanged after a rejected rollback");
}

#[test]
fn recover_restores_a_whole_volatile_db_rollback() {
    let mut state = make_state();
    let point1 = point(100, 1);
    let point2 = point(200, 2);

    forward_to(&mut state, point1, 1);
    forward_to(&mut state, point2, 2);
    assert_eq!(*state.tip(), point2);

    #[derive(Debug, thiserror::Error)]
    #[error("failed to fetch block")]
    pub struct FailedToFetchBlock;

    // Rolling back to the immutable tip clears the whole volatile DB
    assert_no_rollback_to_with_blocks(
        &mut state,
        &Point::Origin,
        std::iter::once(Err(anyhow!(FailedToFetchBlock))),
        |err| assert!(matches!(err, FailedToFetchBlock)),
    );

    // Asserting recovering restored the entire volatile DB, including the tip.
    assert_eq!(*state.tip(), point2, "tip is restored after recovering a whole volatileDB rollback");
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
        None,
        VecDeque::new(),
    )
}

/// Forward the ledger to a given point
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

impl ReadStore for MockStore {
    fn tip(&self) -> amaru_ledger::store::Result<Point> {
        Ok(Point::Origin)
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
