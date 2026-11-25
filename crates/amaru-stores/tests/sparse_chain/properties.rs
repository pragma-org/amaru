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

//! This test executes random roll forwards and fork switches on a sparse chain, from the last block
//! of epoch e to the end of epoch e+1. Then one block in epoch e+2 crosses the boundary that joins
//! the computation and releases the snapshot.
//!
//! The random steps stay in epoch e+1: after the forced flush at the start of epoch e+2, a rollback
//! can go past a stable block, and such a rollback is not legal.
//!
//! Slots are relative to the first slot of epoch e+1. Every chain starts with a fragment at -3 and
//! the last block of epoch e at -2. The stake distribution of epoch e is due at the stability
//! window, 129600, and epoch e+2 starts at 432000.
//!
//! During the test a candidate snapshot might be taken, and/or finalized and/or put aside during a
//! fork then recovered. Eventually the state on disk must match the state declared by the
//! ledger latest snapshot. A failed fork switch must also leave the ledger as it was.

#![expect(clippy::unwrap_used, clippy::panic)]

use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use amaru_kernel::{Block, Epoch, PREPROD_GLOBAL_PARAMETERS, Point};
use amaru_ledger::state::{ForkSwitchOutcome, State};
use amaru_stores::rocksdb::RocksDBHistoricalStores;
use amaru_uplc::arena_pool::ArenaPool;
use proptest::{collection::vec, prelude::*, test_runner::RngSeed};

use crate::{
    MockStore, empty_block_at, epoch_of, forward_to, invalid_block_at, make_state_in_epoch_with_snapshots_and_store,
    roll_forward,
};

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, rng_seed: RngSeed::Fixed(42), ..ProptestConfig::default() })]

    #[test]
    fn a_sparse_chain_keeps_its_candidate_directories_and_survives_failed_switches(steps in vec(step(), 1..10)) {
        let mut ledger = Ledger::new();
        for step in steps {
            // make sure that a failed switch is properly recovered
            let before = ledger.state();
            if !ledger.run(step) {
                prop_assert_eq!(ledger.state(), before, "a failed switch doesn't change the ledger state");
            }
            ledger.check_candidate_directories();
        }

        ledger.cross_the_boundary();
        ledger.check_candidate_directories();
        prop_assert_eq!(ledger.state.candidate_snapshot_on_disk(), None, "the boundary into epoch e+2 releases the snapshot");
    }
}

const EPOCH_LENGTH: i64 = 432_000;

#[derive(Debug, Clone)]
enum Step {
    /// One block per gap after the tip.
    Forward(Vec<i64>),
    /// Roll back `rollback` blocks, then one block per gap: as many blocks, or one more.
    Switch { rollback: usize, gaps: Vec<i64>, invalid: bool },
}

/// Return a step modifying the ledger: either we roll forward with some possible gaps between slots
/// or we roll forward a series of contiguous slots, or we switch to a new fork.
/// The new fork might be valid or invalid.
fn step() -> impl Strategy<Value = Step> {
    let window = u64::from(PREPROD_GLOBAL_PARAMETERS.stability_window()) as i64;
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param as usize;
    let gap = move || prop_oneof![1..20i64, window..window + 5];
    let switch = (0..=6usize).prop_flat_map(move |rollback| {
        (vec(gap(), rollback.max(1)..=rollback + 1), any::<bool>()).prop_map(move |(gaps, invalid)| Step::Switch {
            rollback,
            gaps,
            invalid,
        })
    });
    prop_oneof![
        3 => gap().prop_map(|gap| Step::Forward(vec![gap])),
        1 => Just(Step::Forward(vec![1; k])),
        3 => switch,
    ]
}

/// What the ledger answers about its chain. A failed fork switch must not change these answers.
#[derive(Debug, PartialEq)]
struct LedgerState {
    tip: Point,
    candidate: Option<(Epoch, Point)>,
    most_recent_snapshot: Epoch,
    stable: Vec<Point>,
}

struct Ledger {
    state: State<MockStore, RocksDBHistoricalStores>,
    /// The blocks written to the stable store.
    stable: Arc<Mutex<Vec<Point>>>,
    boundary: u64,
    chain: Vec<i64>,
    /// The directory of the store, removed with the store.
    directory: PathBuf,
}

impl Ledger {
    /// A ledger in epoch e, with a fragment at -3, the last block of epoch e at -2, and a first
    /// block in epoch e+1 at 0.
    fn new() -> Self {
        let epoch = epoch_of(69_200_000);
        let boundary = (69_200_000..).find(|slot| epoch_of(*slot) > epoch).unwrap();
        assert_eq!(epoch_of(boundary + EPOCH_LENGTH as u64), epoch + 2);

        let store = MockStore::new();
        let directory = store.cfg.dir.clone();
        let (mut state, stable) =
            make_state_in_epoch_with_snapshots_and_store(epoch, &[epoch - 3, epoch - 2, epoch - 1], store);
        forward_to(&mut state, empty_block_at(boundary - 3).point());
        let mut ledger = Self { state, stable, boundary, chain: vec![-3], directory };
        ledger.extend(&[-2, 0]);
        ledger
    }

    /// Run a step, and return true when it is a failed fork switch. A step that reaches epoch e+2,
    /// or rolls back the fragment, is skipped.
    /// Return true if the run succeeded or false if a switch failed.
    fn run(&mut self, step: Step) -> bool {
        match step {
            Step::Forward(gaps) => {
                if let Some(slots) = self.after(self.chain.len(), &gaps) {
                    self.extend(&slots);
                }
                true
            }
            Step::Switch { rollback, gaps, invalid } => self.switch(rollback, &gaps, invalid),
        }
    }

    /// Extend the chain by rolling forward blocks associated to a list of slots.
    fn extend(&mut self, slots: &[i64]) {
        for slot in slots {
            let block = self.block(*slot);
            roll_forward(&mut self.state, &block);
        }
        self.chain.extend(slots);
    }

    /// Roll forward a block in epoch e+2. A fork switch can leave the tip in epoch e: then a block
    /// at slot 0 goes first, so that the chain always crosses epoch e+1.
    fn cross_the_boundary(&mut self) {
        if self.chain.last().is_some_and(|slot| *slot < 0) {
            self.extend(&[0]);
        }
        self.extend(&[EPOCH_LENGTH + 1]);
    }

    /// Rollback a number of slots, and switch to a new fork with new gaps
    /// If invalid is true, roll forward an invalid block at the end of the new fork to fail the switch.
    /// Return false when the switch failed.
    fn switch(&mut self, rollback: usize, gaps: &[i64], invalid: bool) -> bool {
        if rollback >= self.chain.len() {
            return false;
        }
        let kept = self.chain.len() - rollback;
        let Some(fork) = self.after(kept, gaps) else { return true };
        let fork_point = self.block(self.chain[kept - 1]).point();
        let mut blocks: Vec<_> = fork.iter().map(|slot| self.block(*slot)).collect();
        if invalid {
            *blocks.last_mut().unwrap() = invalid_block_at(self.absolute(*fork.last().unwrap()));
        }

        let outcome = self
            .state
            .switch_to_fork(&fork_point, blocks, &ArenaPool::new(1024, 0))
            .unwrap_or_else(|error| panic!("the switch at {fork_point:?} to {fork:?} failed: {error:#}"));
        let adopted = match outcome {
            ForkSwitchOutcome::Completed { .. } => fork.len(),
            ForkSwitchOutcome::Partial { .. } => fork.len() - 1,
            ForkSwitchOutcome::Failed { .. } => return false,
        };
        self.chain.truncate(kept);
        self.chain.extend(&fork[..adopted]);
        true
    }

    /// Return the elements of the ledger state that we want to verify
    fn state(&self) -> LedgerState {
        LedgerState {
            tip: self.state.tip().into_owned(),
            candidate: self.state.candidate_snapshot(),
            most_recent_snapshot: self.state.most_recent_snapshot(),
            stable: self.stable.lock().unwrap().clone(),
        }
    }

    /// Rollback blocks so that we only keep `kept` blocks.
    /// Return one slot per gap after the first `kept` blocks of the chain,
    /// or `None` when they reach epoch e+2.
    fn after(&self, kept: usize, gaps: &[i64]) -> Option<Vec<i64>> {
        let mut slot = self.chain[kept - 1];
        let slots: Vec<i64> = gaps
            .iter()
            .map(|gap| {
                slot += gap;
                slot
            })
            .collect();
        (slot < EPOCH_LENGTH).then_some(slots)
    }

    fn block(&self, slot: i64) -> Block {
        empty_block_at(self.absolute(slot))
    }

    fn absolute(&self, slot: i64) -> u64 {
        self.boundary.checked_add_signed(slot).unwrap()
    }

    /// Check that the candidate snapshot declared by the ledger LatestSnapshot as being on disk
    /// (either because a computation is running it or because it hasn't been finalized yet),
    /// is indeed the only one staying on disk.
    fn check_candidate_directories(&self) {
        let actual_candidate_snapshots_on_disk: BTreeSet<String> = fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".candidate-"))
            .collect();

        let expected_candidate_snapshot_on_disk: Option<String> = self
            .state
            .candidate_snapshot_on_disk()
            .map(|(epoch, last_block)| format!("{epoch}.candidate-{}", last_block.hash()));

        assert_eq!(
            actual_candidate_snapshots_on_disk,
            expected_candidate_snapshot_on_disk.into_iter().collect(),
            "the candidate snapshot referenced as the latest snapshot is exactly the one remaining on disk if it exists"
        );
    }
}
