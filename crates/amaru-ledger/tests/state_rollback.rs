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
    BlockHeight, EraHistory, GlobalParameters, Hash, NetworkName, Point, ProtocolParameters, Slot, Tip,
};
use amaru_ledger::{
    state::{BackwardError, State, VolatileState},
    store::GovernanceActivity,
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

#[test]
fn rollback_to_a_volatile_common_ancestor_succeeds() {
    let mut state = make_state();
    let earlier = point(100, 1);
    let later = point(200, 2);

    forward_to(&mut state, earlier, 1);
    forward_to(&mut state, later, 2);
    assert_eq!(*state.tip(), later);

    state.rollback_to(&earlier).unwrap();
    assert_eq!(*state.tip(), earlier);
}

#[test]
fn rollback_before_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(50, 9);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(BackwardError::UnknownRollbackPoint { rollback_point, .. }) if rollback_point == to,
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
        Err(BackwardError::UnknownRollbackPoint { rollback_point, .. }) if rollback_point == to,
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
        Err(BackwardError::UnknownRollbackPoint { rollback_point, .. }) if rollback_point == to,
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
        Err(BackwardError::RollbackPointInFuture { rollback_point, .. }) if rollback_point == to,
    ));
    assert_eq!(*state.tip(), point(100, 1), "tip is unchanged after a rejected rollback");
}

// HELPERS

/// Create an initial ledger state
#[expect(clippy::expect_used)]
fn make_state() -> State<RocksDB, RocksDBHistoricalStores> {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = <&EraHistory>::from(network).clone();
    let global_parameters: GlobalParameters = <&GlobalParameters>::from(network).clone();
    let protocol_parameters: ProtocolParameters =
        <&ProtocolParameters>::try_from(network).expect("preprod parameters available").clone();

    let tmp = tempfile::tempdir().expect("tempdir creation succeeds");
    let cfg = RocksDbConfig::new(tmp.path().to_path_buf());
    let store = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");
    let snapshots = RocksDBHistoricalStores::new(&cfg, 0);

    State::new_with(
        store,
        snapshots,
        network,
        era_history,
        global_parameters,
        protocol_parameters,
        GovernanceActivity { consecutive_dormant_epochs: 0 },
        VecDeque::new(),
    )
}

/// Forward the ldeger to a given point
#[expect(clippy::expect_used)]
fn forward_to(state: &mut State<RocksDB, RocksDBHistoricalStores>, point: Point, height: u64) {
    let issuer = Hash::new([0u8; 28]);
    state
        .forward(VolatileState::default().anchor(Tip::new(point, BlockHeight::from(height)), issuer))
        .expect("forward");
}

fn point(slot: u64, tag: u8) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([tag; 32]))
}
