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

//! Random roll forwards and fork switches on a sparse chain, from the last block of epoch e to the
//! end of epoch e+1. The forced flush at the start of epoch e+2 is a known exception.
//!
//! Slots are relative to the first slot of epoch e+1. Every chain starts with a fragment at -3 and
//! the last block of epoch e at -2. The stake distribution of epoch e is due at the stability
//! window, 129600, and epoch e+2 starts at 432000.

#![expect(clippy::unwrap_used, clippy::panic)]

use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use amaru_kernel::{Block, Epoch, PREPROD_GLOBAL_PARAMETERS, Point};
use amaru_ledger::state::{ForkSwitchOutcome, PoolSummaries, State};
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
    fn the_candidate_directories_on_disk_are_the_ledgers(steps in vec(step(), 1..10)) {
        let mut ledger = Ledger::new();
        for step in steps {
            ledger.run(step);
            ledger.assert_directories_match();
        }
    }

    #[test]
    fn the_answers_depend_on_the_chain_only_and_a_failed_switch_changes_nothing(steps in vec(step(), 1..10)) {
        let mut ledger = Ledger::new();
        for step in steps {
            let before = matches!(step, Step::Switch { .. }).then(|| ledger.answers());
            if let Some(ForkSwitchOutcome::Failed { .. }) = ledger.run(step) {
                assert_eq!(Some(ledger.answers()), before, "a failed switch changes nothing");
            }
        }

        let mut reference = Ledger::fragment();
        reference.extend(&ledger.chain[1..]);
        assert_eq!(ledger.answers(), reference.answers(), "on the chain {:?}", ledger.chain);
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

/// What the ledger answers about its chain, once the stake distribution computation is done.
#[derive(Debug, PartialEq)]
struct Answers {
    tip: Point,
    candidate: Option<(Epoch, Point)>,
    most_recent_snapshot: Epoch,
    published: Option<PoolSummaries>,
    stable: Vec<Point>,
}

struct Ledger {
    state: State<MockStore, RocksDBHistoricalStores>,
    stable: Arc<Mutex<Vec<Point>>>,
    published: Arc<Mutex<Option<PoolSummaries>>>,
    boundary: u64,
    chain: Vec<i64>,
    directory: RemovedOnDrop,
}

impl Ledger {
    fn new() -> Self {
        let mut ledger = Self::fragment();
        ledger.extend(&[-2, 0]);
        ledger
    }

    /// A ledger in epoch e whose chain is only the fragment at -3.
    fn fragment() -> Self {
        let epoch = epoch_of(69_200_000);
        let boundary = (69_200_000..).find(|slot| epoch_of(*slot) > epoch).unwrap();
        assert_eq!(epoch_of(boundary + EPOCH_LENGTH as u64), epoch + 2);

        let store = MockStore::new();
        let directory = RemovedOnDrop(store.cfg.dir.clone());
        let (mut state, stable) =
            make_state_in_epoch_with_snapshots_and_store(epoch, &[epoch - 3, epoch - 2, epoch - 1], store);
        let published = Arc::new(Mutex::new(None));
        let last = published.clone();
        state.set_on_stake_dist_updated(Arc::new(move |summaries| *last.lock().unwrap() = Some(summaries)));
        forward_to(&mut state, empty_block_at(boundary - 3).point());
        Self { state, stable, published, boundary, chain: vec![-3], directory }
    }

    fn extend(&mut self, slots: &[i64]) {
        for slot in slots {
            let block = self.block(*slot);
            roll_forward(&mut self.state, &block);
        }
        self.chain.extend(slots);
    }

    /// The outcome of the step when it is a fork switch. A step that reaches epoch e+2, or rolls
    /// back the fragment, is skipped.
    fn run(&mut self, step: Step) -> Option<ForkSwitchOutcome> {
        match step {
            Step::Forward(gaps) => {
                if let Some(slots) = self.after(self.chain.len(), &gaps) {
                    self.extend(&slots);
                }
                None
            }
            Step::Switch { rollback, gaps, invalid } => self.switch(rollback, &gaps, invalid),
        }
    }

    fn switch(&mut self, rollback: usize, gaps: &[i64], invalid: bool) -> Option<ForkSwitchOutcome> {
        if rollback >= self.chain.len() {
            return None;
        }
        let kept = self.chain.len() - rollback;
        let fork = self.after(kept, gaps)?;
        let fork_point = self.block(self.chain[kept - 1]).point();
        let mut blocks: Vec<_> = fork.iter().map(|slot| self.block(*slot)).collect();
        if invalid {
            *blocks.last_mut().unwrap() = invalid_block_at(self.absolute(*fork.last().unwrap()));
        }

        let outcome = self
            .state
            .switch_to_fork(&fork_point, blocks, &ArenaPool::new(1024, 0))
            .unwrap_or_else(|error| panic!("the switch at {fork_point:?} to {fork:?} failed: {error:#}"));
        let adopted = match &outcome {
            ForkSwitchOutcome::Completed { .. } => fork.len(),
            ForkSwitchOutcome::Partial { .. } => fork.len() - 1,
            ForkSwitchOutcome::Failed { .. } => return Some(outcome),
        };
        self.chain.truncate(kept);
        self.chain.extend(&fork[..adopted]);
        Some(outcome)
    }

    /// One slot per gap after the first `kept` blocks of the chain, or `None` when they reach
    /// epoch e+2.
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

    fn answers(&self) -> Answers {
        let deadline = Instant::now() + Duration::from_secs(60);
        while self.state.is_computing_stake_distribution() {
            assert!(Instant::now() < deadline, "the stake distribution computation finishes");
            thread::sleep(Duration::from_millis(1));
        }
        Answers {
            tip: self.state.tip().into_owned(),
            candidate: self.state.candidate_snapshot(),
            most_recent_snapshot: self.state.most_recent_snapshot(),
            published: self.published.lock().unwrap().clone(),
            stable: self.stable.lock().unwrap().clone(),
        }
    }

    fn assert_directories_match(&self) {
        let on_disk: BTreeSet<String> = fs::read_dir(&self.directory.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".candidate-"))
            .collect();
        let held: BTreeSet<String> = self
            .state
            .candidate_snapshot_on_disk()
            .map(|(epoch, last_block)| format!("{epoch}.candidate-{}", last_block.hash()))
            .into_iter()
            .collect();
        assert_eq!(on_disk, held, "the candidate directories on disk are the ledger's");
    }
}

/// Declared after the state in [`Ledger`], so that the store is closed before its directory goes.
struct RemovedOnDrop(PathBuf);

impl Drop for RemovedOnDrop {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0)
            && !thread::panicking()
        {
            panic!("removing {:?}: {error}", self.0);
        }
    }
}
