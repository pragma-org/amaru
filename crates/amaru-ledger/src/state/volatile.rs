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

use amaru_kernel::{ComparableProposalId, MemoizedTransactionOutput, Point, PoolId, StakeCredential, TransactionInput};

mod db;
pub use db::{RewardsAtTip, VolatileDB};

mod overlay;

mod fragment;
pub use fragment::{
    AccountBind, AnchoredVolatileFragment, CommitteeMemberBind, DRepBind, Existence, StoreUpdate, VolatileFragment,
};

mod series;
pub use series::VolatileSeries;

mod view;
pub use view::VolatileView;

/// An outward-facing store API to query the volatile as a store.
pub trait VolatileState {
    // --------------------------------------------------------------------------------------- UTxOs
    // TODO: unify this API with the others; we could simply return an 'Existence'
    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput>;
    fn has_consumed_input(&self, input: &TransactionInput) -> bool;

    // --------------------------------------------------------------------------------------- Pools
    type Pool;
    fn resolve_pool(&self, pool_id: PoolId) -> Self::Pool;

    // ------------------------------------------------------------------------------------ Accounts
    type Account;
    fn resolve_account(&self, credential: &StakeCredential) -> Self::Account;

    // --------------------------------------------------------------------------------------- DReps
    type DRep;
    fn resolve_drep(&self, credential: &StakeCredential) -> Self::DRep;

    // ----------------------------------------------------------------------------------- CCMembers
    type CCMember;
    fn resolve_cc_member(&self, credential: &StakeCredential) -> Self::CCMember;

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal;
    fn resolve_proposal(&self, proposal_id: &ComparableProposalId) -> Self::Proposal;
}

/// A sequence-like API used by the VolatileDB and VolatileSeries.
pub trait VolatileSequence {
    type Item;

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
    fn view_back(&self) -> Option<&Self::Item>;
    fn view_front(&self) -> Option<&Self::Item>;
    fn has_point(&self, point: &Point) -> bool;

    fn iter(&self) -> impl Iterator<Item = &Self::Item>;
    fn into_iter(self) -> impl Iterator<Item = Self::Item>;

    fn pop_front(&mut self) -> Option<Self::Item>;
    fn push_back(&mut self, item: Self::Item);

    fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point>;
    fn clear(&mut self);
}

/// Shared test fixtures for the volatile keystone proptests, used by both `series` and `db`: a
/// generator of UTxO-lifecycle windows and the brute-force oracle the maintained aggregate is
/// checked against.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use amaru_kernel::{Hash, MemoizedTransactionOutput, TransactionInput};
    use proptest::prelude::*;

    use crate::{state::diff_set::DiffSet, tests::fake_output};

    pub(crate) const VOLATILE_WINDOW: usize = 6;

    prop_compose! {
        /// A window of [`DiffSet`]s where each tagged UTxO has a unique lifecycle: produced once and
        /// optionally consumed at a strictly later index. Mirrors UTxO uniqueness, so a newest-first
        /// walk has a well-defined answer for every key.
        pub(crate) fn unique_lifecycle_diffs(volatile_window: usize)(
            plan in prop::collection::vec(
                (0usize..volatile_window, prop::option::of(0usize..volatile_window)),
                0..16,
            )
        ) -> Vec<DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>> {
            let mut diffs: Vec<DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>> =
                (0..volatile_window).map(|_| DiffSet::default()).collect();

            for (tag, (produced_at, consume_offset)) in plan.into_iter().enumerate() {
                let tag = tag as u8;
                diffs[produced_at].produce(test_input(tag), Arc::new(fixed_output()));
                if let Some(offset) = consume_offset {
                    let consumed_at = produced_at + 1 + offset;
                    if consumed_at < volatile_window {
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

    /// Brute-force oracle: resolve `input` by walking `diffs` newest -> oldest. First consumed -> `None`,
    /// first produce -> `Some`. The reference the maintained aggregate is checked against.
    pub(crate) fn naive_resolve<'a>(
        diffs: &'a [DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>],
        input: &TransactionInput,
    ) -> Option<&'a Arc<MemoizedTransactionOutput>> {
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
        diffs: &[DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>],
        input: &TransactionInput,
    ) -> bool {
        diffs.iter().any(|diff| diff.consumed.contains(input))
    }
}
