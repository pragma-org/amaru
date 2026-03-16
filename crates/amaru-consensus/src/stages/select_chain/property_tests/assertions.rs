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

use std::collections::BTreeSet;

use amaru_kernel::{HeaderHash, IsHeader, Point, Tip};
use amaru_protocols::store_effects::ResourceHeaderStore;

use super::state::{AssertionFailure, StepAction, TestState};
use crate::stages::select_chain;

impl TestState {
    /// Assert that the current non-invalidated output tip is among the best non-invalidated sent tips.
    pub fn assert_best_known_tip(&self, outputs: &[(Tip, Point)]) -> Option<AssertionFailure> {
        if !matches!(self.last_action(), Some(StepAction::Restart)) {
            return None;
        }

        let current_output_tip = self.current_non_invalidated_tip(outputs);
        let best_sent_tips = self.best_non_invalidated_sent_tips();

        let ok = match (&current_output_tip, best_sent_tips.is_empty()) {
            (None, true) => true,
            (Some(current_tip), false) => best_sent_tips.iter().any(|tip| tip.hash() == current_tip.hash()),
            _ => false,
        };

        self.make_assertion_failure(!ok, outputs, current_output_tip, best_sent_tips)
    }

    fn make_assertion_failure(
        &self,
        failed: bool,
        outputs: &[(Tip, Point)],
        current_output_tip: Option<Tip>,
        best_sent_tips: Vec<Tip>,
    ) -> Option<AssertionFailure> {
        if failed {
            Some(AssertionFailure {
                action: self.last_action().unwrap_or(StepAction::FetchNextFromOrigin),
                current_output_tip,
                best_sent_tips,
                outputs: outputs.to_vec(),
                sent_hashes: self.sent_hashes(),
                invalid_blocks: self.invalid_blocks(),
            })
        } else {
            None
        }
    }

    /// Return the latest advertised downstream tip whose ancestry does not include a known invalid block.
    fn current_non_invalidated_tip(&self, outputs: &[(Tip, Point)]) -> Option<Tip> {
        let mut validity_cache = BTreeSet::new();
        let invalid_blocks = self.invalid_blocks();
        let store = self.store();
        outputs.iter().rev().find_map(|(tip, _)| {
            is_non_invalidated_hash(tip.hash(), &invalid_blocks, store.clone(), &mut validity_cache).then_some(*tip)
        })
    }

    /// Return all non-invalidated sent headers that tie for best under the current chain-selection rules.
    fn best_non_invalidated_sent_tips(&self) -> Vec<Tip> {
        let mut validity_cache = BTreeSet::new();
        let invalid_blocks = self.invalid_blocks();
        let store = self.store();
        let non_invalidated = self
            .sent_hashes()
            .iter()
            .filter_map(|hash| {
                is_non_invalidated_hash(*hash, &invalid_blocks, store.clone(), &mut validity_cache)
                    .then(|| store.load_tip(hash))
                    .flatten()
            })
            .collect::<BTreeSet<_>>();

        let max_tip = non_invalidated
            .iter()
            .max_by(|left, right| {
                select_chain::cmp_tip(
                    store.load_header(&left.hash()).as_ref(),
                    store.load_header(&right.hash()).as_ref(),
                )
            })
            .copied();
        let max_tip_header = max_tip.and_then(|tip| store.load_header(&tip.hash()));

        non_invalidated
            .into_iter()
            .filter(|tip| {
                select_chain::cmp_tip(store.load_header(&tip.hash()).as_ref(), max_tip_header.as_ref()).is_eq()
            })
            .collect()
    }
}

/// Return whether this hash and all known ancestors reachable in the store avoid invalidated blocks.
fn is_non_invalidated_hash(
    hash: HeaderHash,
    invalid_blocks: &BTreeSet<HeaderHash>,
    store: ResourceHeaderStore,
    valid_hashes: &mut BTreeSet<HeaderHash>,
) -> bool {
    let mut current = Some(hash);
    let mut traversed = vec![];

    while let Some(hash) = current {
        if invalid_blocks.contains(&hash) {
            return false;
        }

        if valid_hashes.contains(&hash) {
            valid_hashes.extend(traversed);
            return true;
        }

        let Some(header) = store.load_header(&hash) else {
            return false;
        };

        traversed.push(hash);
        current = header.parent();
    }

    valid_hashes.extend(traversed);
    true
}
