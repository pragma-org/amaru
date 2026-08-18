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
//! Tests for switching the ledger `State` to a new fork.
//! We want to show that:
//!
//!  - The switch is atomic: either the whole fork is applied or nothing is applied.
//!  - If the fork is applied we end up with a longer chain than the one we replaced,
//!    and the ledger tip is updated to the new fork tip.
//!
//!  In practice, this works in conjunction with the consensus layer sending the right data to the ledger.
//!  `switch_to_fork` takes 2 arguments:
//!   - The fork point: that fork must be in the volatile window of the ledger
//!   - The tip of the fork: its height must equal or greater than the current tip of the ledger + 1
//!
//!  The outcome of a switch to fork should thus be:
//!
//!  - Completed: the fork was applied and the ledger tip is updated to the new fork tip.
//!  - Failed: one of the blocks in the fork was invalid, and the ledger tip is restored to the pre-switch tip.
//!
//!  There is however a third outcome, Partial, which is a bit more subtle.
//!  It happens when:
//!   - A snapshot was forced by an epoch transition during the replay of the fork. The note in
//!     `apply_transition` explains that this can only happen if the chain growth property of the
//!     network is breached.
//!   - One of the blocks of the fork then fails to be applied.
//!
//!  In that case we cannot revert the snapshot, and the best we can do is report a "partial" outcome
//!  mentioning the last applied block and the failure.
//!
//!  We follow this test plan:
//!
//!  ASSERTIONS:
//!   1. The fork point is in the volatile window of the ledger.
//!   2. The fork tip is at least as high and at most one block higher than the current ledger tip.
//!
//!  FAILED SWITCH:
//!   1. Small volatile window (< k blocks) + invalid block on the fork.
//!   2. Full volatile window (k blocks) + invalid block on the fork.
//!   3. Full volatile window (k blocks) + fork across and epoch boundary + invalid block after the boundary.
//!   4. Full volatile window (k blocks) + fork across and epoch boundary + valid block after the boundary but failed epoch transition.
//!
//!  COMPLETED SWITCH:
//!   1. Successful fork with no epoch transition but the ledger is not extended (the fork has the same length as the replaced chain).
//!   2. Successful fork with no epoch transition and the ledger is extended (the fork has one more block).
//!   3. Switch to a fork across an epoch transition.
//!
//!  PARTIAL SWITCH:
//!   1. Full volatile window (k blocks) + forced snapshot (chain growth violation) + invalid block on the fork after the snapshot is taken.
//!

use amaru_kernel::{PREPROD_GLOBAL_PARAMETERS, Point};
use amaru_ledger::state::{BackwardError, ForkSwitchOutcome, StateError};
use amaru_plutus::arena_pool::ArenaPool;

use crate::{
    MockStore, StateAcrossEpochBoundary, assert_invalid_switch_to_fork_from, empty_block_at, epoch_of, forward_to,
    invalid_block_at, make_state_across_epoch_boundary, make_state_in_epoch, make_state_in_epoch_with_store, point,
};

// ASSERTIONS
// ----------------------------------------------------------------------------

/// The exhaustive fork-point cases (before the window, unknown slot or hash, in the future) are
/// covered in `rollback.rs`. This test covers a call to switch_to_fork with rejects an incorrect fork
/// point the same way.
#[test]
fn the_fork_point_must_be_in_the_volatile_window() {
    let first_slot = 69_200_000; // inside epoch 163
    let (mut state, _) = make_state_in_epoch(epoch_of(first_slot));
    forward_to(&mut state, point(first_slot + 100));
    forward_to(&mut state, point(first_slot + 200));

    let fork_point = point(first_slot + 150);
    let blocks = vec![empty_block_at(first_slot + 300)];
    assert_invalid_switch_to_fork_from(&mut state, &fork_point, blocks, |err: &BackwardError| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == fork_point))
    });

    assert_eq!(*state.tip(), point(first_slot + 200), "tip is unchanged after a rejected fork point");
}

#[test]
fn the_fork_cannot_exceed_the_replaced_chain_by_more_than_one_block() {
    let first_slot = 69_200_000; // inside epoch 163
    let (mut state, _) = make_state_in_epoch(epoch_of(first_slot));
    forward_to(&mut state, point(first_slot + 100));
    forward_to(&mut state, point(first_slot + 200));

    // Try to switch to a fork rolling back one block and replaying three
    let fork_point = point(first_slot + 100);
    let blocks =
        vec![empty_block_at(first_slot + 300), empty_block_at(first_slot + 400), empty_block_at(first_slot + 500)];
    assert_invalid_switch_to_fork_from(&mut state, &fork_point, blocks, |err: &StateError| {
        assert!(matches!(err, StateError::InvalidForkLength { rollback_length: 1, fork_length: 3 }))
    });

    assert_eq!(*state.tip(), point(first_slot + 200), "tip is unchanged after a rejected fork length");
}

#[test]
fn the_fork_cannot_be_shorter_than_the_replaced_chain() {
    let first_slot = 69_200_000; // inside epoch 163
    let (mut state, _) = make_state_in_epoch(epoch_of(first_slot));
    forward_to(&mut state, point(first_slot + 100));
    forward_to(&mut state, point(first_slot + 200));

    // Roll back two blocks but replay only one.
    let fork_point = Point::Origin;
    let blocks = vec![empty_block_at(first_slot + 300)];
    assert_invalid_switch_to_fork_from(&mut state, &fork_point, blocks, |err: &StateError| {
        assert!(matches!(err, StateError::InvalidForkLength { rollback_length: 2, fork_length: 1 }))
    });

    assert_eq!(*state.tip(), point(first_slot + 200), "tip is unchanged after a rejected fork length");
}

// FAILED SWITCH
// ----------------------------------------------------------------------------

#[test]
fn the_volatile_db_is_restored_when_a_block_on_the_fork_is_invalid_with_small_volatile_window() {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let (mut state, _) = make_state_in_epoch(epoch);

    // The volatile DB holds the whole two-block chain. The immutable tip is Origin:
    //
    //     stable   │       volatile
    //   ---------- | --------------------
    //     Origin   │ point1 ── point2
    //
    let point1 = point(first_slot + 100);
    let point2 = point(first_slot + 200);
    forward_to(&mut state, point1);
    forward_to(&mut state, point2);
    assert_eq!(*state.tip(), point2);

    // Switching to a fork rooted at Origin itself rolls back to the immutable tip, which
    // clears the whole volatile DB (stashed wholesale in the rollback guard), then fails
    // on the second replayed block:
    //
    //     Origin │                                rollback: volatile emptied
    //            └─── block1 ── block2 (invalid)  replay: block1 applies, block2 is rejected
    //
    // Nothing reached the stable store, so recovery restores the whole volatile DB:
    //
    //     Origin │ point1 ── point2
    //
    let block1 = empty_block_at(first_slot + 300);
    let block2 = invalid_block_at(first_slot + 400);
    let block2_tip = block2.point();
    match state.switch_to_fork(&Point::Origin, vec![block1, block2], &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Failed { failure }) => assert_eq!(failure.tip, block2_tip),
        other => panic!("expected a rolled back switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), point2, "tip is restored after recovering a whole volatileDB rollback");
}

#[test]
fn the_volatile_db_is_restored_when_a_block_on_the_fork_is_invalid_with_full_volatile_window() {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let (mut state, stable) = make_state_in_epoch(epoch);

    // Make the volatile window full (`k` blocks).
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;
    for slot in first_slot..(first_slot + k) {
        forward_to(&mut state, point(slot));
    }
    let tip_slot = first_slot + k - 1;
    let original_tip = point(tip_slot);

    // Switch to a fork that goes back two blocks and replays a
    // two-block fork whose second block is invalid:
    //
    //   stable  │              volatile (len k)
    // --------- | ------------------------------------------
    //       ... │ block(first_slot) ── ... ── fork_point ── block(tip_slot − 1) ── block(tip_slot)
    //                                          │
    //                                          └─────────── block1 ─────────────── block2 (invalid)
    let fork_point = point(tip_slot - 2);
    let block1 = empty_block_at(tip_slot + 1);
    let block2 = invalid_block_at(tip_slot + 2);

    let block2_tip = block2.point();
    let blocks = vec![block1, block2];
    let result = state.switch_to_fork(&fork_point, blocks, &ArenaPool::new(1024, 0));

    // Replaying:
    //   rollback 2 blocks -> len k−2
    //   apply block1      -> len k−1   still <= k, nothing is persisted to the stable store
    //   apply block2      -> rejected
    //
    // Since the fork never got longer than the chain it replaced, and nothing reached the stable store
    // the switch is fully undone.
    //
    // ... │ block(first_slot) ── ... ── fork_point ── block(tip_slot − 1) ── block(tip_slot)
    match result {
        Ok(ForkSwitchOutcome::Failed { failure }) => assert_eq!(failure.tip, block2_tip),
        other => panic!("expected a rolled back switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), original_tip, "the pre-switch tip is restored");
    assert!(stable.lock().unwrap().is_empty(), "nothing reached the stable store");
}

#[test]
fn the_volatile_db_is_restored_when_a_block_is_invalid_after_an_epoch_transition() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, block_after } =
        make_state_across_epoch_boundary();

    // Switch to a fork starting below the boundary whose single replayed block is invalid. The
    // rollback re-crosses the boundary but the block after the boundary is invalid
    // rejected by validation:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ --------------------------
    //     fragment ── block1─────────╎ block2            rolled back
    //                      │         ╎
    //                      └─────────╎ block3 (invalid)  replayed
    //
    // The transition only mutated the in-memory state and nothing reached the stable store, so
    // the switch is fully undone, epoch transition included.
    let block3 = invalid_block_at(boundary_slot + 1);
    let block3_tip = block3.point();
    let result = state.switch_to_fork(&block_before.point(), vec![block3], &ArenaPool::new(1024, 0));

    match result {
        Ok(ForkSwitchOutcome::Failed { failure }) => assert_eq!(failure.tip, block3_tip),
        other => panic!("expected a rolled back switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), block_after.point(), "the pre-switch tip is restored");
    assert!(stable.lock().unwrap().is_empty(), "nothing reached the stable store");
}

#[test]
fn the_volatile_db_is_restored_when_there_is_a_failed_epoch_transition() {
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;

    // Find the first epoch boundary after an arbitrary anchor slot, then lay the volatile window
    // over the `k` slots leading up to it:
    //
    //                  epoch e                                          ╎     epoch e+1
    // ----------------------------------------------------------------  ╎ -----------------
    //  ... ── anchor ── ... ── first_slot ── ... ── block(boundary − 1) ╎ boundary ── ...
    //                          └──────── volatile window (k) ─────────┘ ╎
    //
    // so the whole window sits inside a single epoch and the very next slot starts the next one.
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let mut boundary = first_slot + 1;
    while epoch_of(boundary) == epoch {
        boundary += 1;
    }
    let first_slot = boundary - k;
    assert_eq!(epoch_of(first_slot), epoch, "the whole window must sit inside a single epoch");

    // Set a ledger state with a store that will fail the epoch transition.
    let (mut state, stable) = make_state_in_epoch_with_store(epoch, MockStore::failing_transition_progress());
    // Initialize the ledger with roll forwards
    for slot in first_slot..boundary {
        forward_to(&mut state, point(slot));
    }
    let initial_tip = point(boundary - 1);

    // Switch to a fork that goes back one block and replays a single block sitting on the
    // other side of the epoch boundary:
    //
    //                  epoch e                                                 ╎     epoch e+1
    // ------------------------------------------------------------------------ ╎ -----------------
    // stable  │               volatile (len k)                                 ╎
    //     ... │ block(first_slot) ── ... ── fork_point ── block(boundary − 1)  ╎
    //                                        │                                 ╎
    //                                        └──────────────────────────────── ╎ ── replayed
    let fork_point = point(boundary - 2);
    let replayed = empty_block_at(boundary);

    let err = match state.switch_to_fork(&fork_point, std::iter::once(replayed), &ArenaPool::new(1024, 0)) {
        Ok(outcome) => panic!("expected the epoch transition to fail, got: {outcome:?}"),
        Err(err) => err,
    };
    assert!(matches!(err.downcast_ref::<StateError>(), Some(StateError::Storage(_))), "unexpected error: {err:#}");

    assert_eq!(*state.tip(), initial_tip, "the pre-switch tip is restored after a failed transition");
    assert!(stable.lock().unwrap().is_empty(), "nothing reached the stable store");
}

// COMPLETED SWITCH
// ----------------------------------------------------------------------------

#[test]
fn a_switch_to_fork_can_be_successful_without_extending_the_ledger() {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let (mut state, stable) = make_state_in_epoch(epoch);

    // Make the volatile window full (`k` blocks).
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;
    for slot in first_slot..(first_slot + k) {
        forward_to(&mut state, point(slot));
    }
    let tip_slot = first_slot + k - 1;

    // Switch to a fork replacing the tip block with a competing block at the same height (a
    // tie-break between two chains of equal length):
    //
    //   stable  │              volatile (len k)
    // --------- | ------------------------------------------
    //       ... │ block(first_slot) ── ... ── fork_point ── block(tip_slot)
    //                                          │
    //                                          └─────────── block1
    //
    // The window never grows past `k`, so nothing is persisted to the stable store.
    let fork_point = point(tip_slot - 1);
    let block1 = empty_block_at(tip_slot + 1);
    let block1_tip = block1.point();
    match state.switch_to_fork(&fork_point, vec![block1], &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { metrics }) => assert_eq!(metrics.slot, tip_slot + 1),
        other => panic!("expected a completed switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), block1_tip, "the ledger follows the fork tip");
    assert!(stable.lock().unwrap().is_empty(), "nothing reached the stable store");
}

#[test]
fn a_switch_to_fork_can_be_successful_and_extend_the_ledger() {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let (mut state, stable) = make_state_in_epoch(epoch);

    // Make the volatile window full (`k` blocks).
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;
    for slot in first_slot..(first_slot + k) {
        forward_to(&mut state, point(slot));
    }
    let tip_slot = first_slot + k - 1;

    // Switch to a fork that goes back one block and replays two: the fork extends the ledger by
    // one block, the shape chain selection produces for every real fork switch:
    //
    //   stable  │              volatile (len k)
    // --------- | ------------------------------------------
    //       ... │ block(first_slot) ── ... ── fork_point ── block(tip_slot)
    //                                          │
    //                                          └─────────── block1 ────────── block2
    let fork_point = point(tip_slot - 1);
    let block1 = empty_block_at(tip_slot + 1);
    let block2 = empty_block_at(tip_slot + 2);
    let block2_tip = block2.point();

    // Replaying:
    //   rollback block(tip_slot) -> len k−1
    //   apply block1             -> len k    window merely full again, nothing persisted
    //   apply block2             -> len k    block(first_slot) is persisted to the stable store
    match state.switch_to_fork(&fork_point, vec![block1, block2], &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { metrics }) => assert_eq!(metrics.slot, tip_slot + 2),
        other => panic!("expected a completed switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), block2_tip, "the ledger follows the fork tip");
    assert_eq!(
        *stable.lock().unwrap(),
        vec![point(first_slot)],
        "nothing was persisted before the committing block, and exactly one block with it"
    );
}

#[test]
fn a_switch_to_fork_can_be_successful_across_an_epoch_transition() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, .. } =
        make_state_across_epoch_boundary();

    // Switch to a fork starting below the boundary. The rollback re-crosses it, downgrading the
    // effective rewards back to computed ones; replaying block3 re-runs the epoch transition,
    // which must consume those preserved rewards (the background computation is now gone):
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ --------------------------
    //     fragment ── block1         ╎ block2         rolled back
    //                      │         ╎
    //                      └─────────╎ block3         replayed
    let block3 = empty_block_at(boundary_slot + 1);
    let block3_tip = block3.point();
    let result = state.switch_to_fork(&block_before.point(), vec![block3], &ArenaPool::new(1024, 0));

    match result {
        Ok(ForkSwitchOutcome::Completed { .. }) => (),
        other => panic!("expected the re-crossing switch to complete, got: {other:?}"),
    }
    assert_eq!(*state.tip(), block3_tip, "the ledger follows the fork across the boundary");
    assert!(stable.lock().unwrap().is_empty(), "nothing reached the stable store");
}

// PARTIAL SWITCH
// ----------------------------------------------------------------------------

#[test]
fn a_partial_switch_happens_when_a_snapshot_is_forced_during_the_replay() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, .. } =
        make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());

    // Switch to a fork starting below the boundary, whose first block sits a whole stability
    // window (3·k/f slots) into the new epoch — fewer than `k` blocks in that many slots is a
    // chain growth violation. Applying block3 re-runs the epoch transition and, with the
    // pre-boundary tail still unstable, forces it to the stable store: the tail blocks are
    // flushed and a snapshot is taken (see the note in `apply_transition`).
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2                          rolled back
    //                      │         ╎
    //                      └─────────╎ ── ... ── block3 ── block4 (invalid)  replayed
    //                                   3·k/f ──┘
    //
    // block4 then fails, but the snapshot cannot be reverted: the switch keeps the applied
    // prefix and reports a partial outcome.
    let block3 = empty_block_at(boundary_slot + stability_window);
    let block4 = invalid_block_at(boundary_slot + stability_window + 1);
    let block3_tip = block3.point();
    let block4_tip = block4.point();

    let result = state.switch_to_fork(&block_before.point(), vec![block3, block4], &ArenaPool::new(1024, 0));

    match result {
        Ok(ForkSwitchOutcome::Partial { applied_tip, failure, .. }) => {
            assert_eq!(applied_tip, block3_tip);
            assert_eq!(failure.tip, block4_tip);
        }
        other => panic!("expected a partial switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), block3_tip, "the ledger stays on the last applied block");
    assert_eq!(
        *stable.lock().unwrap(),
        vec![point(boundary_slot - 3), block_before.point()],
        "the pre-boundary tail was force-flushed to the stable store, oldest first"
    );
}
