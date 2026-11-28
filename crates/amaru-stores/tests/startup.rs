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

//! Tests for the ledger state right after the node starts.
//!
//! The stake distribution and rewards computation of an epoch starts 3·k/f slots into that epoch.
//! A node can start after that point, so the ledger must start the computation before it applies
//! the first block: this block can cross the epoch boundary, which needs the computed rewards.

use amaru_ledger::store::EpochTransitionProgress;

use crate::{
    MockStore, empty_block_at, epoch_of, make_state_in_epoch_with_snapshots_and_store, make_state_on_start, point,
    roll_forward,
};

#[test]
fn the_first_block_after_a_start_past_the_stability_window_can_cross_the_epoch_boundary() {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let mut boundary_slot = first_slot + 1;
    while epoch_of(boundary_slot) == epoch {
        boundary_slot += 1;
    }

    // The stable tip is the last block of epoch e, so it is after the stability window (3·k/f
    // slots) of epoch e. The volatile state is empty, as after each start. The next block is the
    // first block of epoch e+1:
    //
    //             epoch e                       ╎   epoch e+1
    //     ------------------------------------- ╎ -------------
    //     stable tip                            ╎ block
    //     slots              3·k/f   b−1        ╎ b
    let stable_tip = point(boundary_slot - 1);
    let mock_store = MockStore::new();
    mock_store.stable.lock().unwrap().push(stable_tip);
    let (mut state, _) =
        make_state_in_epoch_with_snapshots_and_store(epoch, &[epoch - 3, epoch - 2, epoch - 1], mock_store);
    assert_eq!(*state.tip(), stable_tip);
    state.start().unwrap();

    // The epoch transition from e to e+1 needs the rewards computed from the snapshots of e−1 and e−3.
    let block = empty_block_at(boundary_slot);
    roll_forward(&mut state, &block);
    assert_eq!(*state.tip(), block.point());
}

#[test]
fn the_first_block_after_a_start_can_resume_an_epoch_transition_that_took_its_snapshot() {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let mut boundary_slot = first_slot + 1;
    while epoch_of(boundary_slot) == epoch {
        boundary_slot += 1;
    }

    // The node stopped in the transition out of epoch e, after the stable store took the snapshot
    // of e. A ledger bootstrapped from a snapshot starts in the same state. The next block is the
    // first block of epoch e+1, so it resumes that transition:
    //
    //             epoch e                       ╎   epoch e+1
    //     ------------------------------------- ╎ -------------
    //     stable tip, snapshot of e             ╎ block
    //     slots                      b−1        ╎ b
    let stable_tip = point(boundary_slot - 1);
    let mock_store = MockStore::with_transition_progress(EpochTransitionProgress::SnapshotTaken);
    mock_store.stable.lock().unwrap().push(stable_tip);
    let (mut state, _) = make_state_on_start(epoch, &[epoch - 2, epoch - 1, epoch], mock_store);
    assert_eq!(state.most_recent_snapshot(), epoch);
    state.start().unwrap();

    let block = empty_block_at(boundary_slot);
    roll_forward(&mut state, &block);
    assert_eq!(*state.tip(), block.point());
    assert_eq!(state.most_recent_snapshot(), epoch);
}
