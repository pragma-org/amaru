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
    store::test_utils::{MockHistoricalStores, MockStore},
};

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
fn make_state() -> State<MockStore, MockHistoricalStores> {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let global_parameters: GlobalParameters = PREPROD_GLOBAL_PARAMETERS.clone();
    let protocol_parameters: ProtocolParameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();

    State::new_with(
        MockStore::new(Point::Origin),
        MockHistoricalStores::new(vec![]),
        Epoch::default(),
        network,
        era_history,
        global_parameters,
        protocol_parameters,
        GovernanceActivity::default(),
        VecDeque::new(),
    )
}

/// Forward the ledger to a given point
#[expect(clippy::expect_used)]
fn forward_to(state: &mut State<MockStore, MockHistoricalStores>, point: Point, height: u64) {
    let issuer = Hash::new([0u8; 28]);
    let tip = Tip::new(point, BlockHeight::from(height));
    state.push_fragment(VolatileFragment::default().anchor(tip, issuer)).expect("forward");
}

fn point(slot: u64, tag: u8) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([tag; 32]))
}
