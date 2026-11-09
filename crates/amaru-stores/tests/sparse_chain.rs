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
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    time::Duration,
};

use amaru_kernel::{Epoch, PREPROD_GLOBAL_PARAMETERS, Point};
use amaru_ledger::{
    LedgerObservers, LedgerStateSnapshot, SnapshotFinality,
    state::{ForkSwitchOutcome, PoolSummaries, State},
};
use amaru_stores::rocksdb::RocksDBHistoricalStores;
use amaru_uplc::arena_pool::ArenaPool;

use crate::{
    MockStore, StateAcrossEpochBoundary, empty_block_at, epoch_of, invalid_block_at, make_state_across_epoch_boundary,
    point, roll_forward,
};

#[test]
fn a_candidate_snapshot_is_taken_after_the_stability_window_without_writing_to_the_stable_store() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, .. } =
        make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());
    let closing_epoch = epoch_of(block_before.slot_or_default().into());
    assert_eq!(state.most_recent_snapshot(), closing_epoch - 1);

    // The first block after the stability window takes a candidate snapshot of epoch e. It also
    // starts the background computation, which reads the candidate snapshot: it computes the
    // stake distribution of epoch e, and publishes it. No other block is necessary, because the
    // next header can be in epoch e+2 and need this distribution:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3
    //                                          3·k/f ──┘
    let published = record_publications(&mut state);
    let block3 = empty_block_at(boundary_slot + stability_window);
    roll_forward(&mut state, &block3);
    assert_eq!(*state.tip(), block3.point());
    assert_eq!(state.most_recent_snapshot(), closing_epoch, "the candidate snapshot counts as the snapshot of epoch e");

    let summaries = next_publication(&published);
    assert!(summaries.by_epoch.contains_key(&closing_epoch), "the stake distribution of epoch e is available");
    assert_eq!(
        summaries.candidates.get(&closing_epoch),
        Some(&block_before.point()),
        "the stake distribution of epoch e is a candidate snapshot valid only for headers that descend from its last block"
    );

    assert_eq!(
        state.volatile_epoch_tail().last(),
        Some(&block_before.point()),
        "the last block of epoch e is still volatile"
    );
    assert!(stable.lock().unwrap().is_empty(), "no block was written to the stable store");
}

#[test]
fn the_stake_distribution_of_a_candidate_snapshot_becomes_final_when_the_last_block_of_its_epoch_is_stable() {
    let SparseChain { mut state, boundary_slot, stability_window, closing_epoch, published, .. } =
        make_sparse_chain_with_candidate_snapshot();

    // block3 is the first block after the stability window: it takes the candidate snapshot of
    // epoch e, and starts the background task that reads this snapshot. There is no block between
    // block2 and block3.
    //
    // block1, the last block of epoch e, becomes stable when `k` blocks follow it. block2, block3
    // and block4 are 3 of these blocks, so `k - 3` more blocks, from slot b+w+2, make block1 stable:
    //
    //       epoch e−1    ╎          epoch e           ╎                     epoch e+1
    //     -------------- ╎ -------------------------- ╎ ---------------------------------------------------
    //     stable blocks  ╎ fragment ── block1         ╎ block2 ── ... ── block3 ── block4 ── k - 3 blocks
    //     slots          ╎                            ╎ b                b+w       b+w+1     b+w+2, …
    //                    ╎                            ╎ └─ stability window ──┘
    //
    // b is `boundary_slot`, the first slot of epoch e+1, and w is `stability_window` (3·k/f slots).
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;
    let first_slot_after_block4 = boundary_slot + stability_window + 2;

    // The last added block should finalize the snapshot since we already have 3 block in epoch e+1:
    // block2, block3 and block4
    for offset in 0..k - 3 {
        roll_forward(&mut state, &empty_block_at(first_slot_after_block4 + offset));
    }

    let summaries = next_publication(&published);
    assert!(summaries.by_epoch.contains_key(&closing_epoch), "the stake distribution of epoch e is still available");
    assert!(summaries.candidates.is_empty(), "the stake distribution of epoch e is final: {:?}", summaries.candidates);
}

#[test]
fn the_last_blocks_of_an_epoch_are_forced_to_the_stable_store_after_a_whole_epoch_with_fewer_than_k_blocks() {
    let SparseChain { mut state, boundary_slot, closing_epoch, published, block1, stable, .. } =
        make_sparse_chain_with_candidate_snapshot();
    assert!(!stable.lock().unwrap().contains(&block1), "block1 is still volatile");

    // Epoch e+1 ends with fewer than `k` blocks, so block1 is still volatile at the boundary of
    // epoch e+2. The first block of epoch e+2 forces block1 to the stable store:
    //
    //             epoch e            ╎                epoch e+1                ╎   epoch e+2
    //     -------------------------- ╎ --------------------------------------- ╎ -------------
    //     fragment ── block1         ╎ block2 ── ... ── block3 ── block4       ╎ block5
    //     slots                      ╎ b                b+w       b+w+1        ╎ b'
    //
    // b is `boundary_slot`, w is `stability_window` (3·k/f slots), and b' is the first slot of
    // epoch e+2.
    let next_boundary_slot = boundary_slot + PREPROD_GLOBAL_PARAMETERS.epoch_length();
    assert_eq!(epoch_of(next_boundary_slot), closing_epoch + 2, "b' is the first slot of epoch e+2");
    roll_forward(&mut state, &empty_block_at(next_boundary_slot));

    assert!(stable.lock().unwrap().contains(&block1), "the forced flush wrote block1 to the stable store");
    assert_eq!(state.candidate_snapshot(), None, "the candidate snapshot of epoch e is removed");
    assert_eq!(state.most_recent_snapshot(), closing_epoch, "the snapshot of epoch e is now a final snapshot");

    let summaries = std::iter::from_fn(|| published.recv_timeout(Duration::from_secs(60)).ok())
        .find(|summaries| summaries.candidates.is_empty());
    assert!(
        summaries.is_some_and(|summaries| summaries.by_epoch.contains_key(&closing_epoch)),
        "the stake distribution of epoch e is published as final"
    );
}

#[test]
fn the_ledger_snapshot_observer_sees_a_candidate_snapshot_as_a_candidate() {
    let StateAcrossEpochBoundary { mut state, boundary_slot, block_before, .. } = make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());
    let closing_epoch = epoch_of(block_before.slot_or_default().into());

    let (sender, observed) = mpsc::channel();
    state.set_observers(LedgerObservers::new().on_ledger_snapshot(move |snapshot: LedgerStateSnapshot<'_>| {
        let _ = sender.send((snapshot.summary.stake_distribution.epoch, snapshot.finality));
    }));

    // block3 takes the candidate snapshot of epoch e, and starts the background task that reads it.
    roll_forward(&mut state, &empty_block_at(boundary_slot + stability_window));

    let (epoch, finality) =
        observed.recv_timeout(Duration::from_secs(60)).expect("the ledger snapshot of epoch e is observed");
    assert_eq!(epoch, closing_epoch);
    assert_eq!(finality, SnapshotFinality::Candidate { last_block: block_before.point() });
}

#[test]
fn a_rollback_past_the_last_block_of_an_epoch_writes_no_abandoned_block_to_the_stable_store() {
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
    let closing_epoch = epoch_of(block_before.slot_or_default().into());
    assert_eq!(
        state.candidate_snapshot(),
        Some((closing_epoch, block_before.point())),
        "block3 takes a candidate snapshot of epoch e, which ends at block1"
    );

    // A competing chain forks at fragment, the block just before block1, the last block of epoch e.
    // The rollback removes three blocks, fewer than `k`, so it is legal:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3                       rolled back
    //     b-3         b-2            ╎ b                b+w
    //            │                   ╎
    //            └──── block1'       ╎ block2' ── block3' ── ... ── block4'          replayed
    //                  b-1           ╎ b+1        b+2               b+w+1
    //
    // b is `boundary_slot` and w is `stability_window` (3·k/f slots). There is no block where the
    // diagram shows `...`.
    let fork_point = point(boundary_slot - 3);
    let fork = vec![
        empty_block_at(boundary_slot - 1),
        empty_block_at(boundary_slot + 1),
        empty_block_at(boundary_slot + 2),
        empty_block_at(boundary_slot + stability_window + 1),
    ];
    let fork_tip = fork.last().unwrap().point();
    let fork_block1 = fork[0].point();

    match state.switch_to_fork(&fork_point, fork, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { .. }) => (),
        other => panic!("expected the switch past the end of epoch e to complete, got: {other:?}"),
    }

    assert_eq!(*state.tip(), fork_tip, "the ledger follows the fork");
    assert_eq!(
        state.candidate_snapshot(),
        Some((closing_epoch, fork_block1)),
        "block4' takes a new candidate snapshot of epoch e, which ends at block1'"
    );
    let stable = stable.lock().unwrap();
    assert!(
        !stable.contains(&block_before.point()) && !stable.contains(&block_after.point()),
        "the blocks of the abandoned chain never reached the stable store: {stable:?}"
    );
}

#[test]
fn a_rollback_past_the_last_block_of_an_epoch_discards_its_candidate_snapshot() {
    let SparseChain { mut state, boundary_slot, stability_window, closing_epoch, published, .. } =
        make_sparse_chain_with_candidate_snapshot();

    // The fork replaces block1, the last block of epoch e, and has no block after the stability
    // window:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3 ── block4            rolled back
    //            │                   ╎
    //            └──── block1'       ╎ block2' ── block3' ── block4'                replayed
    let fork = vec![
        empty_block_at(boundary_slot - 1),
        empty_block_at(boundary_slot + 1),
        empty_block_at(boundary_slot + 2),
        empty_block_at(boundary_slot + 3),
    ];
    let fork_block1 = fork[0].point();
    match state.switch_to_fork(&point(boundary_slot - 3), fork, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { .. }) => (),
        other => panic!("expected the switch to complete, got: {other:?}"),
    }

    let summaries = next_publication(&published);
    assert!(
        !summaries.by_epoch.contains_key(&closing_epoch) && !summaries.candidates.contains_key(&closing_epoch),
        "the stake distribution of epoch e, which ends at block1, is withdrawn"
    );
    assert_eq!(state.most_recent_snapshot(), closing_epoch - 1, "no snapshot of epoch e is left");

    // After the stability window, the fork gets its own candidate snapshot, which ends with block1'.
    roll_forward(&mut state, &empty_block_at(boundary_slot + stability_window));
    roll_forward(&mut state, &empty_block_at(boundary_slot + stability_window + 1));

    let summaries = next_publication(&published);
    assert!(summaries.by_epoch.contains_key(&closing_epoch), "the fork publishes its stake distribution of epoch e");
    assert_eq!(
        summaries.candidates.get(&closing_epoch),
        Some(&fork_block1),
        "the stake distribution of epoch e is the one of the fork, which ends at block1'"
    );
}

#[test]
fn a_rollback_that_keeps_the_last_block_of_an_epoch_keeps_its_candidate_snapshot() {
    let SparseChain { mut state, boundary_slot, stability_window, closing_epoch, published, block2, .. } =
        make_sparse_chain_with_candidate_snapshot();

    // The fork starts after block2, so block1, the last block of epoch e, stays on the chain:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3 ── block4             rolled back
    //                                ╎           │
    //                                ╎           └──── block3' ── block4'           replayed
    let fork = vec![
        empty_block_at(boundary_slot + stability_window + 2),
        empty_block_at(boundary_slot + stability_window + 3),
    ];
    match state.switch_to_fork(&block2, fork, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { .. }) => (),
        other => panic!("expected the switch to complete, got: {other:?}"),
    }

    assert_eq!(state.most_recent_snapshot(), closing_epoch, "the candidate snapshot of epoch e is kept");
    assert!(
        matches!(published.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "the stake distribution of epoch e is not withdrawn"
    );
}

#[test]
fn a_failed_switch_past_the_last_block_of_an_epoch_restores_its_candidate_snapshot() {
    let SparseChain { mut state, boundary_slot, closing_epoch, published, block1, .. } =
        make_sparse_chain_with_candidate_snapshot();

    // The fork replaces block1, the last block of epoch e, but its last block is invalid:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3 ── block4            restored
    //            │                   ╎
    //            └──── block1'       ╎ block2' ── block3' ── block4' (invalid)      replayed
    let fork = vec![
        empty_block_at(boundary_slot - 1),
        empty_block_at(boundary_slot + 1),
        empty_block_at(boundary_slot + 2),
        invalid_block_at(boundary_slot + 3),
    ];
    match state.switch_to_fork(&point(boundary_slot - 3), fork, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Failed { .. }) => (),
        other => panic!("expected the switch to fail, got: {other:?}"),
    }
    assert_eq!(state.most_recent_snapshot(), closing_epoch, "the candidate snapshot of epoch e is back");

    // The recovery computes the stake distribution of epoch e again, from the restored candidate
    // snapshot. The next header can be in epoch e+2 and need it, so no new block is necessary.
    let summaries = std::iter::from_fn(|| published.recv_timeout(Duration::from_secs(60)).ok())
        .find(|summaries| summaries.by_epoch.contains_key(&closing_epoch));
    assert_eq!(
        summaries.and_then(|summaries| summaries.candidates.get(&closing_epoch).cloned()),
        Some(block1),
        "the stake distribution of epoch e is published again, with no new block"
    );
}

#[test]
fn a_failed_switch_that_takes_another_candidate_snapshot_restores_the_original_one() {
    let SparseChain { mut state, boundary_slot, stability_window, closing_epoch, published, block1, .. } =
        make_sparse_chain_with_candidate_snapshot();

    // The fork replaces block1, the last block of epoch e. Its block3' comes after the stability
    // window, so the replay takes another candidate snapshot of epoch e, which ends at block1'.
    // Then block4' is invalid:
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2 ── ... ── block3 ── block4            restored
    //            │                   ╎
    //            └──── block1'       ╎ block2' ── ... ── block3' ── block4'         replayed
    //     slots        b-1           ╎ b+1               b+w        b+w+1
    //
    // b is `boundary_slot` and w is `stability_window` (3·k/f slots). block4' is invalid.
    let fork = vec![
        empty_block_at(boundary_slot - 1),
        empty_block_at(boundary_slot + 1),
        empty_block_at(boundary_slot + stability_window),
        invalid_block_at(boundary_slot + stability_window + 1),
    ];
    match state.switch_to_fork(&point(boundary_slot - 3), fork, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Failed { .. }) => (),
        other => panic!("expected the switch to fail, got: {other:?}"),
    }

    // The recovery removes the candidate snapshot of the replay, and restores the original one.
    assert_eq!(
        state.candidate_snapshot(),
        Some((closing_epoch, block1)),
        "the candidate snapshot that ends at block1 is back, not the one that ends at block1'"
    );

    // The computation from the replayed snapshot can publish before the recovery cancels it. The
    // recovery then computes the stake distribution of epoch e again, from the original snapshot.
    let summaries = std::iter::from_fn(|| published.recv_timeout(Duration::from_secs(60)).ok())
        .find(|summaries| summaries.candidates.get(&closing_epoch) == Some(&block1));
    assert!(
        summaries.is_some_and(|summaries| summaries.by_epoch.contains_key(&closing_epoch)),
        "the stake distribution of epoch e, which ends at block1, is published again"
    );
}

#[test]
fn a_failed_switch_is_fully_restored_when_a_candidate_snapshot_is_taken_during_the_replay() {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, block_after } =
        make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());

    // Switch to a fork starting below the boundary, whose first block sits a whole stability
    // window (3·k/f slots) into the new epoch — fewer than `k` blocks in that many slots is a
    // chain growth violation. Applying block3 re-runs the epoch transition and, with the
    // pre-boundary tail still unstable, takes a candidate snapshot of epoch e. The stable store
    // is unchanged.
    //
    //             epoch e            ╎        epoch e+1
    //     -------------------------- ╎ -------------------------------------------
    //     fragment ── block1         ╎ block2                          rolled back
    //                      │         ╎
    //                      └─────────╎ ── ... ── block3 ── block4 (invalid)  replayed
    //                                   3·k/f ──┘
    //
    // block4 then fails. Nothing reached the stable store, so the switch restores the chain it
    // replaced.
    let block3 = empty_block_at(boundary_slot + stability_window);
    let block4 = invalid_block_at(boundary_slot + stability_window + 1);
    let block4_tip = block4.point();

    let result = state.switch_to_fork(&block_before.point(), vec![block3, block4], &ArenaPool::new(1024, 0));

    match result {
        Ok(ForkSwitchOutcome::Failed { failure, .. }) => assert_eq!(failure.tip, block4_tip),
        other => panic!("expected a failed switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), block_after.point(), "the ledger is back on the chain it had before the switch");
    assert!(stable.lock().unwrap().is_empty(), "nothing reached the stable store");

    let closing_epoch = epoch_of(block_before.slot_or_default().into());
    assert_eq!(state.candidate_snapshot(), None, "the candidate snapshot taken during the replay is removed");
    assert_eq!(state.most_recent_snapshot(), closing_epoch - 1, "no snapshot of epoch e is left");
}

// HELPERS

/// A ledger state on a sparse chain, after the stake distribution of epoch e was computed from a
/// candidate snapshot and published:
///
///             epoch e            ╎        epoch e+1
///     -------------------------- ╎ -------------------------------------------
///     fragment ── block1         ╎ block2 ── ... ── block3 ── block4
///     slots                      ╎ b                b+w       b+w+1
///                                ╎ └─ stability window ──┘
///
/// b is `boundary_slot` and w is `stability_window` (3·k/f slots). block3, the first block after
/// the stability window, takes the candidate snapshot of epoch e, and starts the background task
/// that reads this snapshot.
struct SparseChain {
    state: State<MockStore, RocksDBHistoricalStores>,
    stability_window: u64,
    /// The first slot of epoch e+1, where block2 is.
    boundary_slot: u64,
    /// Epoch e, whose last block is block1.
    closing_epoch: Epoch,
    /// The publications of stake distributions, after the one of epoch e.
    published: Receiver<PoolSummaries>,
    /// The last block of epoch e.
    block1: Point,
    /// The first block of epoch e+1.
    block2: Point,
    /// The blocks written to the stable store.
    stable: Arc<Mutex<Vec<Point>>>,
}

fn make_sparse_chain_with_candidate_snapshot() -> SparseChain {
    let StateAcrossEpochBoundary { mut state, stable, boundary_slot, block_before, block_after } =
        make_state_across_epoch_boundary();
    let stability_window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window());
    let closing_epoch = epoch_of(block_before.slot_or_default().into());
    let published = record_publications(&mut state);

    roll_forward(&mut state, &empty_block_at(boundary_slot + stability_window));
    roll_forward(&mut state, &empty_block_at(boundary_slot + stability_window + 1));

    let summaries = next_publication(&published);
    assert!(summaries.by_epoch.contains_key(&closing_epoch), "the stake distribution of epoch e is available");
    assert_eq!(
        summaries.candidates.get(&closing_epoch),
        Some(&block_before.point()),
        "the stake distribution of epoch e ends at block1"
    );

    SparseChain {
        state,
        boundary_slot,
        stability_window,
        closing_epoch,
        published,
        block1: block_before.point(),
        block2: block_after.point(),
        stable,
    }
}

/// Record each stake distribution publication.
fn record_publications(state: &mut State<MockStore, RocksDBHistoricalStores>) -> Receiver<PoolSummaries> {
    let (sender, receiver) = mpsc::channel();
    state.set_on_stake_dist_updated(Arc::new(move |summaries: PoolSummaries| {
        let _ = sender.send(summaries);
    }));
    receiver
}

#[expect(clippy::expect_used)]
fn next_publication(published: &Receiver<PoolSummaries>) -> PoolSummaries {
    published.recv_timeout(Duration::from_secs(60)).expect("a stake distribution is published")
}
