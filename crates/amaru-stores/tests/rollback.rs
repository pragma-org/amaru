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

//! Tests for ledger `State` rollback against the RocksDB store backend.
//! Those are "unit tests" in the sense that they test the rollback functionality of the `State` struct,
//! knowing that rollback is only ever called during a switch to fork.

use amaru_kernel::{Hash, Point};
use amaru_ledger::state::BackwardError;

use crate::{assert_no_rollback_to, forward_to, make_state, point, point_with_hash, rollback_to};

#[test]
fn rollback_to_a_volatile_common_ancestor_succeeds() {
    let mut state = make_state();
    let earlier = point(100);
    let later = point(200);

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
    forward_to(&mut state, point(100), 1);
    forward_to(&mut state, point(200), 2);

    let to = point(50);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_hash_is_rejected() {
    let mut state = make_state();
    let point1 = point_with_hash(100, Hash::new([1u8; 32]));
    let point2 = point_with_hash(200, Hash::new([2u8; 32]));
    forward_to(&mut state, point1, 1);
    forward_to(&mut state, point2, 2);

    let to = point_with_hash(100, Hash::new([3u8; 32]));
    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(
            matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to),
            "expected an UnknownRollbackPoint error for {to:?}, got: {err:?}"
        )
    });

    assert_eq!(*state.tip(), point2, "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_slot_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100), 1);
    forward_to(&mut state, point(200), 2);

    let to = point(150);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_after_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100), 1);

    let to = point(101);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::RollbackPointInFuture { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(100), "tip is unchanged after a rejected rollback");
}
