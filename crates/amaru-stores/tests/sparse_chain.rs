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

//! Tests for chains that violate the chain growth property: fewer than `k` blocks in the first
//! stability window (3·k/f slots) of an epoch.
//!
//! The snapshot of an epoch can only be taken from the stable store once the last blocks of that
//! epoch are `k` blocks deep. On a sparse chain, they are not, at the time when the stake
//! distribution of that epoch is needed. The ledger must still produce the snapshot without
//! writing blocks that are not final to the stable store, so that a legal rollback past these
//! blocks stays possible.

use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use amaru_kernel::PREPROD_GLOBAL_PARAMETERS;
use amaru_ledger::state::{ForkSwitchOutcome, PoolSummaries};
use amaru_uplc::arena_pool::ArenaPool;

use crate::{
    StateAcrossEpochBoundary, empty_block_at, epoch_of, make_state_across_epoch_boundary, point, roll_forward,
};

#[test]
fn a_rollback_past_the_last_blocks_of_an_epoch_is_possible_after_the_stability_window() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, block_after } =
        make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());

    // The first block after the boundary is followed by a single block, a whole stability window
    // later. That is far fewer than `k` blocks, so the last blocks of epoch e are still volatile
    // when the ledger needs the snapshot of epoch e:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3
    //                                          3·k/f ──┘
    let block3 = empty_block_at(boundary_slot + stability_window);
    roll_forward(&mut state, &block3);
    assert_eq!(*state.tip(), block3.point());

    // A competing chain forks before block1, the last block of epoch e. The rollback removes three
    // blocks, fewer than `k`, so it is legal:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3                       rolled back
    //            │                   ╎
    //            └──── block1'       ╎ block2' ── block3' ── ... ── block4'          replayed
    //                                                     3·k/f ──┘
    let fork_point = point(boundary_slot - 3);
    let fork = vec![
        empty_block_at(boundary_slot - 1),
        empty_block_at(boundary_slot + 1),
        empty_block_at(boundary_slot + 2),
        empty_block_at(boundary_slot + stability_window + 1),
    ];
    let fork_tip = fork[3].point();

    match state.switch_to_fork(&fork_point, fork, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { .. }) => (),
        other => panic!("expected the switch past the end of epoch e to complete, got: {other:?}"),
    }

    assert_eq!(*state.tip(), fork_tip, "the ledger follows the fork");
    let stable = stable.lock().unwrap();
    assert!(
        !stable.contains(&block_before.point()) && !stable.contains(&block_after.point()),
        "the blocks of the abandoned chain never reached the stable store: {stable:?}"
    );
}

#[test]
fn a_candidate_snapshot_is_taken_after_the_stability_window_without_writing_to_the_stable_store() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, .. } =
        make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());
    let closing_epoch = epoch_of(block_before.point().slot_or_default().into());
    assert_eq!(state.most_recent_snapshot(), closing_epoch - 1);

    // The first block after the stability window takes a candidate snapshot of epoch e. It also
    // starts the background computation, which reads the candidate snapshot: it computes the
    // stake distribution of epoch e, and publishes it:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3
    //                                          3·k/f ──┘
    let (published, distributions) = mpsc::channel();
    state.set_on_stake_dist_updated(Arc::new(move |summaries: PoolSummaries| {
        let _ = published.send(summaries.by_epoch.keys().copied().collect::<Vec<_>>());
    }));
    let block3 = empty_block_at(boundary_slot + stability_window);
    roll_forward(&mut state, &block3);
    assert_eq!(*state.tip(), block3.point());
    assert_eq!(state.most_recent_snapshot(), closing_epoch, "the candidate snapshot counts as the snapshot of epoch e");

    let epochs = distributions.recv_timeout(Duration::from_secs(60)).expect("a stake distribution is published");
    assert!(epochs.contains(&closing_epoch), "the stake distribution of epoch e is available: {epochs:?}");

    assert!(stable.lock().unwrap().is_empty(), "the last blocks of epoch e are still volatile");
}
