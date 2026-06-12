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

//! Shared test fixtures for the volatile keystone proptests, used by both `series` and `db`: a
//! generator of UTxO-lifecycle windows and the brute-force oracle the maintained aggregate is
//! checked against.

use amaru_kernel::{Hash, MemoizedTransactionOutput, TransactionInput};
use proptest::prelude::*;

use crate::{state::diff_set::DiffSet, tests::fake_output};

pub(crate) const VOLATILE_WINDOW: usize = 6;

prop_compose! {
    /// A window of [`DiffSet`]s where each tagged UTxO has a unique lifecycle: produced once and
    /// optionally consumed at a strictly later index. Mirrors UTxO uniqueness, so a newest-first
    /// walk has a well-defined answer for every key.
    pub(crate) fn unique_lifecycle_diffs()(
        plan in prop::collection::btree_map(
            0u8..16,
            (0usize..VOLATILE_WINDOW, prop::option::of(0usize..VOLATILE_WINDOW)),
            0..16,
        )
    ) -> Vec<DiffSet<TransactionInput, MemoizedTransactionOutput>> {
        let mut diffs: Vec<DiffSet<TransactionInput, MemoizedTransactionOutput>> =
            (0..VOLATILE_WINDOW).map(|_| DiffSet::default()).collect();

        for (tag, (produced_at, consume_offset)) in plan {
            diffs[produced_at].produce(test_input(tag), fixed_output());
            if let Some(offset) = consume_offset {
                let consumed_at = produced_at + 1 + offset;
                if consumed_at < VOLATILE_WINDOW {
                    diffs[consumed_at].consume(test_input(tag));
                }
            }
        }

        diffs
    }
}

pub(crate) fn test_input(tag: u8) -> TransactionInput {
    TransactionInput { transaction_id: Hash::new([tag; 32]), index: 0 }
}

pub(crate) fn fixed_output() -> MemoizedTransactionOutput {
    fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335")
}

/// Brute-force oracle: resolve `input` by walking `diffs` newest→oldest. First consumed ⇒ `None`,
/// first produce ⇒ `Some`. The reference the maintained aggregate is checked against.
pub(crate) fn naive_resolve<'a>(
    diffs: &'a [DiffSet<TransactionInput, MemoizedTransactionOutput>],
    input: &TransactionInput,
) -> Option<&'a MemoizedTransactionOutput> {
    for diff in diffs.iter().rev() {
        if diff.consumed.contains(input) {
            return None;
        }
        if let Some(output) = diff.produced.get(input) {
            return Some(output);
        }
    }
    None
}

pub(crate) fn naive_has_consumed(
    diffs: &[DiffSet<TransactionInput, MemoizedTransactionOutput>],
    input: &TransactionInput,
) -> bool {
    diffs.iter().any(|diff| diff.consumed.contains(input))
}
