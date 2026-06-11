// Copyright 2024 PRAGMA
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

use std::{collections::VecDeque, sync::Arc};

use amaru_kernel::{MemoizedTransactionOutput, Point, StakeCredential, TransactionInput};

use crate::{
    context::AccountState,
    state::{AnchoredVolatileFragment, VolatileFragment, drep_state::DRepState},
};

#[derive(Default)]
pub struct VolatileDB {
    sequence: VecDeque<AnchoredVolatileFragment>,
    aggregate: VolatileFragment,
}

impl VolatileDB {
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    pub fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.back()
    }

    pub fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.front()
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&Arc<MemoizedTransactionOutput>> {
        if self.aggregate.utxo.consumed.contains(input) {
            return None;
        }
        self.aggregate.utxo.produced.get(input)
    }

    pub fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.aggregate.utxo.consumed.contains(input)
    }

    pub fn resolve_drep(&self, credential: &StakeCredential) -> Option<&Arc<DRepState>> {
        if self.aggregate.dreps.consumed.contains(credential) {
            return None;
        }
        self.aggregate.dreps.produced.get(credential)
    }

    pub fn has_unregistered_drep(&self, credential: &StakeCredential) -> bool {
        self.aggregate.dreps.consumed.contains(credential)
    }

    pub fn resolve_account(&self, credential: &StakeCredential) -> Option<&Arc<AccountState>> {
        if self.aggregate.accounts.consumed.contains(credential) {
            return None;
        }
        self.aggregate.accounts.produced.get(credential)
    }

    pub fn has_unregistered_account(&self, credential: &StakeCredential) -> bool {
        self.aggregate.accounts.consumed.contains(credential)
    }

    pub fn contains(&self, point: &Point) -> bool {
        self.sequence.binary_search_by_key(point, |anchored| anchored.point()).is_ok()
    }

    pub fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        let stabilized = self.sequence.pop_front();
        if stabilized.is_some() {
            self.recompute_aggregate();
        }
        stabilized
    }

    pub fn push_back(&mut self, fragment: AnchoredVolatileFragment) {
        self.aggregate.compose(&fragment.fragment);
        self.sequence.push_back(fragment);
    }

    fn recompute_aggregate(&mut self) {
        let mut aggregate = VolatileFragment::default();
        for AnchoredVolatileFragment { fragment, .. } in self.sequence.iter() {
            aggregate.compose(fragment);
        }
        self.aggregate = aggregate;
    }

    pub fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point> {
        let target_slot = point.slot_or_default();

        // Check if the target point is beyond the sequence
        // In this case we simply return Ok since it this would not change the volatile fragment.
        if let Some(last) = self.sequence.back()
            && last.slot() < target_slot
        {
            tracing::warn!(
                name: "rollback_to.beyond",
                %target_slot,
                last_slot = ?last.slot(),
                "Attempting to rollback to a point beyond the last known volatile fragment"
            );
            return Ok(());
        }

        // Check if the target point is before the sequence
        // In this case we return an error since it means rolling back the stable DB
        if let Some(first) = self.sequence.front()
            && target_slot < first.slot()
        {
            tracing::error!(
                name: "rollback_to.before",
                %target_slot,
                first_slot = ?first.slot(),
                "Attempting to rollback to a point before the first point of the volatile fragment"
            );
            return Err(point);
        }

        // Now we know the target point is within the sequence.
        // Keep all elements with point <= target point.
        let mut ix = 0;
        let mut found = false;
        for diff in self.sequence.iter() {
            if diff.point() <= *point {
                ix += 1;
                if diff.point() == *point {
                    found = true;
                    break;
                }
            } else {
                return Err(point);
            }
        }

        if !found {
            return Err(point);
        }

        self.sequence.truncate(ix);
        self.recompute_aggregate();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.sequence.clear();
        self.aggregate = VolatileFragment::default();
    }

    pub fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment> {
        self.sequence.iter()
    }
}

#[cfg(test)]
impl VolatileDB {
    pub fn fixture() -> Self {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));
        assert_eq!(db.len(), 3);
        db
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{Hash, Point, Slot};
    use proptest::prelude::*;

    use super::*;
    use crate::state::diff_set::DiffSet;

    const VOLATILE_WINDOW: usize = 6;

    prop_compose! {
        fn unique_lifecycle_diffs()(
            plan in prop::collection::btree_map(
                0u8..16,
                (0usize..VOLATILE_WINDOW, prop::option::of(0usize..VOLATILE_WINDOW)),
                0..16,
            )
        ) -> Vec<DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>> {
            let mut diffs: Vec<DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>> =
                (0..VOLATILE_WINDOW).map(|_| DiffSet::default()).collect();

            for (tag, (produced_at, consume_offset)) in plan {
                diffs[produced_at].produce(test_input(tag), Arc::new(fixed_output()));
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

    proptest! {
        #[test]
        fn aggregated_lookups_match_naive_walk(diffs in unique_lifecycle_diffs()) {
            let mut db = VolatileDB::default();
            for (index, diff) in diffs.iter().enumerate() {
                let mut anchored = AnchoredVolatileFragment::fixture(index as u64, index as u8);
                anchored.fragment.utxo = diff.clone();
                db.push_back(anchored);
            }

            for tag in 0u8..16 {
                let input = test_input(tag);
                prop_assert_eq!(db.resolve_input(&input).is_some(), naive_resolve(&diffs, &input).is_some());
                prop_assert_eq!(db.has_consumed_input(&input), naive_has_consumed(&diffs, &input));
            }
        }
    }

    proptest! {
        #[test]
        fn aggregated_lookups_match_naive_walk_after_stabilization(diffs in unique_lifecycle_diffs()) {
            let mut db = VolatileDB::default();
            for (index, diff) in diffs.iter().enumerate() {
                let mut anchored = AnchoredVolatileFragment::fixture(index as u64, index as u8);
                anchored.fragment.utxo = diff.clone();
                db.push_back(anchored);
            }

            db.pop_front();
            let remaining = &diffs[1..];

            for tag in 0u8..16 {
                let input = test_input(tag);
                prop_assert_eq!(db.resolve_input(&input).is_some(), naive_resolve(remaining, &input).is_some());
                prop_assert_eq!(db.has_consumed_input(&input), naive_has_consumed(remaining, &input));
            }
        }
    }

    #[test]
    fn test_rollback_to_point_before_sequence_fails() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 5 (before the first element at slot 10)
        // This represents rolling back to a point in the stable DB
        let rollback_point = Point::Specific(Slot::from(5), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        // This should fail
        // (rolling back to a point inside the stable DB is not allowed)
        assert!(result.is_err());
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_exact_last_element_should_succeed() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 30 (the last element)
        let rollback_point = Point::Specific(Slot::from(30), Hash::new([0u8; 32]));

        // This should succeed, keeping all 3 elements
        let result = db.rollback_to(&rollback_point);

        assert!(result.is_ok(), "Rolling back to the exact slot of the last element should succeed");
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_middle_element_succeeds() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 20 (middle element)
        let rollback_point = Point::Specific(Slot::from(20), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        // This should succeed
        assert!(result.is_ok());
        assert_eq!(db.len(), 2, "Should keep elements at slots 10 and 20");
    }

    #[test]
    fn test_rollback_to_point_after_sequence_succeeds() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Try to rollback to slot 40 (after the sequence)
        let rollback_point = Point::Specific(Slot::from(40), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        // This should succeed
        assert!(result.is_ok(), "Rolling back to a point after the sequence should succeed");
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_slot_between_elements_succeeds() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 25 (between 20 and 30)
        let rollback_point = Point::Specific(Slot::from(25), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        assert_eq!(result.unwrap_err(), &rollback_point);
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_consumed_input_is_tracked() {
        let input = test_input(1);
        let mut anchored = AnchoredVolatileFragment::fixture(10, 1);
        anchored.fragment.utxo.consume(input.clone());

        let mut db = VolatileDB::default();
        db.push_back(anchored);

        assert!(db.has_consumed_input(&input));
        assert!(db.resolve_input(&input).is_none());
    }

    #[test]
    fn test_rollback_removes_consumed_input_from_cache() {
        let input = test_input(1);
        let mut db = VolatileDB::default();
        let first = AnchoredVolatileFragment::fixture(10, 1);
        let first_point = first.point();
        db.push_back(first);

        let mut second = AnchoredVolatileFragment::fixture(20, 2);
        second.fragment.utxo.consume(input.clone());
        db.push_back(second);

        assert!(db.has_consumed_input(&input));

        db.rollback_to(&first_point).unwrap();

        assert!(!db.has_consumed_input(&input));
    }

    // HELPERS

    fn test_input(tag: u8) -> TransactionInput {
        TransactionInput { transaction_id: Hash::new([tag; 32]), index: 0 }
    }

    fn fixed_output() -> MemoizedTransactionOutput {
        crate::tests::fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335")
    }

    fn naive_resolve<'a>(
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

    fn naive_has_consumed(
        diffs: &[DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>],
        input: &TransactionInput,
    ) -> bool {
        diffs.iter().any(|diff| diff.consumed.contains(input))
    }
}
