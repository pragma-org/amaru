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

use std::{collections::BTreeSet, sync::Arc};

use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, Point, Tip};
use amaru_ouroboros_traits::{ChainStore, ReadOnlyChainStore, in_memory_consensus_store::InMemConsensusStore};
use amaru_protocols::store_effects::ResourceHeaderStore;
use rand::{Rng, SeedableRng, prelude::IndexedRandom, rngs::StdRng};

use super::harness::Harness;
use crate::{
    headers_tree::data_generation::{GeneratedTree, generate_tree_of_headers},
    stages::select_chain,
};

const MAX_CONSECUTIVE_NON_TIP_STEPS: usize = 24;

/// Oracle and generator state for the `select_chain` property test.
///
/// It tracks which generated headers have been sent to the stage, how they have been
/// validated so far, and enough progress state to choose the next action to execute on the stage.
///
pub struct TestState {
    /// Shared store mutated by the oracle side of the property test.
    store: ResourceHeaderStore,
    /// Full generated header tree from which upstream tips are chosen.
    generated_tree: GeneratedTree,
    /// RNG used to choose the next action and validation result.
    rng: StdRng,
    /// Oracle bookkeeping for which generated headers have been sent and validated.
    headers_state: HeadersState,
    /// Last generated action. This is used to drive startup and restart-specific assertions.
    last_action: Option<StepAction>,
    /// Number of consecutive restart/validation steps since the last new tip.
    /// This lets the generator force eventual progress toward sending another
    /// upstream tip instead of looping forever on restarts and validation results.
    consecutive_non_tip_steps: usize,
}

impl TestState {
    /// Build the oracle state from a generated tree and a deterministic RNG seed.
    pub fn new(generated_tree: GeneratedTree, seed: u64, store: ResourceHeaderStore) -> Self {
        Self {
            store,
            generated_tree,
            headers_state: HeadersState::new(),
            consecutive_non_tip_steps: 0,
            last_action: None,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// Pick the next randomized action and apply its effects immediately to the local state.
    ///
    /// The non-tip counter makes sure that we eventually send a new upstream tip.
    ///
    pub fn generate_random_action(&mut self) -> StepAction {
        let action = if self.last_action.is_none() {
            StepAction::FetchNextFromOrigin
        } else if self.consecutive_non_tip_steps >= MAX_CONSECUTIVE_NON_TIP_STEPS {
            let headers = self.available_new_tips();
            let header = headers.choose(&mut self.rng).unwrap().clone();
            StepAction::TipFromUpstream(header.hash())
        } else {
            let headers_pending_validation = self.headers_pending_validation();
            // Weight the random choice toward validation and restart once a chain has
            // multiple non-validated headers. This makes generated runs spend time in the
            // interesting "partially validated fork" states without giving up tip progress.
            let validation_weight = if headers_pending_validation.is_empty() {
                0
            } else {
                // If there are blocks pending validation, enable block validation result messages to be generated,
                // and bias further toward them when blocks pending validation outnumbers still-unsent tips.
                let mut validation_weight = 1;
                if self.has_multi_step_pending_chain() {
                    validation_weight += 2;
                }
                if self.headers_state.pending_validation.len() >= self.available_new_tips().len() {
                    validation_weight += 1;
                }
                validation_weight
            };

            let mut choices = vec![ActionChoice::TipFromUpstream, ActionChoice::Restart];
            choices.extend(std::iter::repeat_n(ActionChoice::BlockValidationResult, validation_weight));

            match choices.choose(&mut self.rng).copied().unwrap() {
                ActionChoice::TipFromUpstream => {
                    let headers = self.available_new_tips();
                    let header = headers.choose(&mut self.rng).unwrap().clone();
                    StepAction::TipFromUpstream(header.hash())
                }
                ActionChoice::BlockValidationResult => {
                    let hash = *headers_pending_validation.choose(&mut self.rng).unwrap();
                    let valid = self.rng.random_bool(0.5);
                    StepAction::BlockValidationResult(hash, valid)
                }
                ActionChoice::Restart => StepAction::Restart,
            }
        };

        self.execute_action(&action);
        action
    }

    /// Apply the oracle-side store and bookkeeping updates associated with the chosen action.
    fn execute_action(&mut self, action: &StepAction) {
        self.last_action = Some(action.clone());
        match action {
            StepAction::FetchNextFromOrigin => {}
            StepAction::TipFromUpstream(hash) => {
                let header = self.generated_header(*hash);
                self.store.store_header(&header).unwrap();
                if header.parent().is_none() && self.store.get_anchor_hash() != header.hash() {
                    self.store.set_anchor_hash(&header.hash()).unwrap();
                    self.store.set_best_chain_hash(&header.hash()).unwrap();
                }
                self.headers_state.mark_sent(*hash);
                self.reset_non_tip_steps();
            }
            StepAction::BlockValidationResult(hash, valid) => {
                self.headers_state.mark_validated(*hash, *valid);
                // once a block has been validated the best_chain_hash in the store might be set to a new value.
                self.recompute_best_chain_hash();
                self.increment_non_tip_steps();
            }
            StepAction::Restart => self.increment_non_tip_steps(),
        }
    }

    pub fn store(&self) -> ResourceHeaderStore {
        self.store.clone()
    }

    pub fn last_action(&self) -> Option<StepAction> {
        self.last_action.clone()
    }

    pub fn sent_hashes(&self) -> BTreeSet<HeaderHash> {
        self.headers_state.sent_hashes.clone()
    }

    pub fn invalid_blocks(&self) -> BTreeSet<HeaderHash> {
        self.headers_state.invalid_blocks.clone()
    }

    /// Generate the next action, or `None` once the randomized scenario is exhausted.
    pub fn next_action(&mut self) -> Option<StepAction> {
        self.has_actions().then(|| self.generate_random_action())
    }

    /// Return headers whose parent has already been sent, so they are eligible as the next upstream tip.
    fn available_new_tips(&self) -> Vec<BlockHeader> {
        self.generated_tree
            .nodes()
            .into_iter()
            .filter(|header| {
                !self.headers_state.sent_hashes.contains(&header.hash())
                    && header.parent().map(|parent| self.headers_state.sent_hashes.contains(&parent)).unwrap_or(true)
            })
            .collect()
    }

    /// Recover a generated header by hash before it has been stored in the shared store.
    fn generated_header(&self, hash: HeaderHash) -> BlockHeader {
        self.generated_tree.nodes().into_iter().find(|header| header.hash() == hash).unwrap()
    }

    /// Measure how many still-pending headers remain on this chain suffix.
    fn pending_chain_length(&self, hash: HeaderHash) -> usize {
        let mut current = Some(hash);
        let mut pending_len = 0;

        while let Some(hash) = current {
            if self.headers_state.pending_validation.contains(&hash) {
                pending_len += 1;
            }

            current = self.store.load_header(&hash).and_then(|header| header.parent());
        }

        pending_len
    }

    /// Keep the store's persisted best-chain pointer aligned with the best fully validated sent tip.
    fn recompute_best_chain_hash(&mut self) {
        let best_tip = self
            .headers_state
            .sent_hashes
            .iter()
            .filter(|hash| self.is_fully_validated_non_invalidated_hash(**hash))
            .max_by(|left, right| {
                select_chain::cmp_tip(self.store.load_header(left).as_ref(), self.store.load_header(right).as_ref())
            })
            .copied();

        if let Some(best_tip) = best_tip {
            self.store.set_best_chain_hash(&best_tip).unwrap();
        }
    }

    /// Check whether this hash belongs to a fully validated chain with no invalidated ancestor.
    fn is_fully_validated_non_invalidated_hash(&self, hash: HeaderHash) -> bool {
        let mut current = Some(hash);

        while let Some(hash) = current {
            if self.headers_state.invalid_blocks.contains(&hash)
                || self.headers_state.pending_validation.contains(&hash)
            {
                return false;
            }

            current = self.store.load_header(&hash).and_then(|header| header.parent());
        }

        true
    }

    /// Return true if there is at least one tip that was sent downstream with a list of at least 2 headers pending validation in its ancestry.
    /// This is used to bias the generator toward producing more validation and restart steps.
    fn has_multi_step_pending_chain(&self) -> bool {
        self.headers_state.sent_hashes.iter().copied().any(|hash| self.pending_chain_length(hash) >= 2)
    }

    /// Header hashes that correspond to not-yet-validated blocks.
    fn headers_pending_validation(&self) -> Vec<HeaderHash> {
        self.headers_state.pending_validation.iter().copied().collect()
    }

    /// Continue until the startup fetch has been issued and there are no more eligible new tips to reveal.
    pub fn has_actions(&self) -> bool {
        self.last_action.is_none() || !self.available_new_tips().is_empty()
    }

    fn increment_non_tip_steps(&mut self) {
        self.consecutive_non_tip_steps += 1;
    }

    fn reset_non_tip_steps(&mut self) {
        self.consecutive_non_tip_steps = 0;
    }
}

/// Internal bookkeeping for which headers have been sent and how their blocks have been validated so far.
struct HeadersState {
    /// All headers that have been sent upstream to the stage at least once.
    sent_hashes: BTreeSet<HeaderHash>,
    /// Subset of `sent_hashes` that still await a validation result.
    pending_validation: BTreeSet<HeaderHash>,
    /// Headers that have been marked invalid by block validation results.
    invalid_blocks: BTreeSet<HeaderHash>,
    /// Headers that have been marked valid by generated validation results.
    valid_blocks: BTreeSet<HeaderHash>,
}

impl HeadersState {
    fn new() -> Self {
        Self {
            sent_hashes: BTreeSet::new(),
            pending_validation: BTreeSet::new(),
            invalid_blocks: BTreeSet::new(),
            valid_blocks: BTreeSet::new(),
        }
    }

    fn mark_sent(&mut self, hash: HeaderHash) {
        self.sent_hashes.insert(hash);
        self.pending_validation.insert(hash);
    }

    fn mark_validated(&mut self, hash: HeaderHash, valid: bool) {
        self.pending_validation.remove(&hash);
        if valid {
            self.valid_blocks.insert(hash);
        } else {
            self.invalid_blocks.insert(hash);
        }
    }
}

/// Selector for what should be the next generated action
#[derive(Clone, Copy)]
enum ActionChoice {
    TipFromUpstream,
    BlockValidationResult,
    Restart,
}

/// The actions we want to execute with the select_chain stage either correspond to sending a message
/// or restarting the stage from the store.
#[derive(Debug, Clone)]
pub enum StepAction {
    FetchNextFromOrigin,
    TipFromUpstream(HeaderHash),
    BlockValidationResult(HeaderHash, bool),
    Restart,
}

#[derive(Debug)]
pub struct AssertionFailure {
    pub action: StepAction,
    pub current_output_tip: Option<Tip>,
    pub best_sent_tips: Vec<Tip>,
    pub outputs: Vec<(Tip, Point)>,
    pub sent_hashes: BTreeSet<HeaderHash>,
    pub invalid_blocks: BTreeSet<HeaderHash>,
}

impl std::fmt::Display for AssertionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "after {:?}, current output tip {:?} is not among best non-invalidated sent tips {:?}",
            self.action, self.current_output_tip, self.best_sent_tips
        )
    }
}

/// Check that generated action sequences eventually make the stage switch to a different fork.
#[test]
fn test_generated_action_sequences_eventually_switch_to_a_fork() {
    let fork_switch = (0..256).any(find_fork_switch_execution);
    assert!(fork_switch, "generated action sequences eventually switch to a different fork");
}

/// Check that generated action sequences eventually make the stage fall back to an intermediate ancestor.
#[test]
fn test_generated_action_sequences_eventually_fall_back_to_an_intermediate_ancestor() {
    let fallback = (0..256).any(find_intermediate_fallback_execution);
    assert!(fallback, "generated action sequences eventually fall back to an intermediate ancestor");
}

fn find_fork_switch_execution(seed: u64) -> bool {
    let generated_tree = generate_tree_of_headers(seed, 6);
    let store = Arc::new(InMemConsensusStore::default());
    let mut harness = Harness::new(store.clone());
    let mut state = TestState::new(generated_tree, seed, store.clone());
    let mut previous_output: Option<Tip> = None;

    while let Some(action) = state.next_action() {
        for (tip, _) in harness.execute_action(&action, &state) {
            if let Some(previous_tip) = previous_output
                && are_on_different_forks(store.as_ref(), previous_tip.hash(), tip.hash())
            {
                return true;
            }
            previous_output = Some(tip);
        }
    }

    false
}

fn find_intermediate_fallback_execution(seed: u64) -> bool {
    let generated_tree = generate_tree_of_headers(seed, 6);
    let store = Arc::new(InMemConsensusStore::default());
    let mut harness = Harness::new(store.clone());
    let mut state = TestState::new(generated_tree, seed, store.clone());
    let mut previous_output: Option<Tip> = None;

    while let Some(action) = state.next_action() {
        for (tip, _) in harness.execute_action(&action, &state) {
            if let Some(previous_tip) = previous_output
                && is_strict_ancestor(store.as_ref(), tip.hash(), previous_tip.hash())
                && tip.hash() != store.get_best_chain_hash()
            {
                return true;
            }
            previous_output = Some(tip);
        }
    }

    false
}

fn are_on_different_forks(store: &dyn ChainStore<BlockHeader>, left: HeaderHash, right: HeaderHash) -> bool {
    left != right && !is_strict_ancestor(store, left, right) && !is_strict_ancestor(store, right, left)
}

fn is_strict_ancestor(store: &dyn ChainStore<BlockHeader>, ancestor: HeaderHash, hash: HeaderHash) -> bool {
    let mut current = store.load_header(&hash).and_then(|header| header.parent());

    while let Some(hash) = current {
        if hash == ancestor {
            return true;
        }
        current = store.load_header(&hash).and_then(|header| header.parent());
    }

    false
}
