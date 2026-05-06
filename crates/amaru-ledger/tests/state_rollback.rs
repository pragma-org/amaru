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
use amaru_stores::in_memory::MemoryStore;

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

    let before_volatile = point(50, 9);
    let err = state.rollback_to(&before_volatile).err().unwrap();

    assert!(
        matches!(err, BackwardError::UnknownRollbackPoint(p) if p == before_volatile),
        "expected UnknownRollbackPoint, got {err:?}",
    );
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

// HELPERS

/// Create an initial ledger state
#[expect(clippy::expect_used)]
fn make_state() -> State<MemoryStore, MemoryStore> {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = <&EraHistory>::from(network).clone();
    let global_parameters: GlobalParameters = <&GlobalParameters>::from(network).clone();
    let protocol_parameters: ProtocolParameters =
        <&ProtocolParameters>::try_from(network).expect("preprod parameters available").clone();

    let store = MemoryStore::new(era_history.clone(), protocol_parameters.clone());
    let snapshots = MemoryStore::new(era_history.clone(), protocol_parameters.clone());

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
fn forward_to(state: &mut State<MemoryStore, MemoryStore>, point: Point, height: u64) {
    let issuer = Hash::new([0u8; 28]);
    state
        .forward(VolatileState::default().anchor(Tip::new(point, BlockHeight::from(height)), issuer))
        .expect("forward");
}

fn point(slot: u64, tag: u8) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([tag; 32]))
}
