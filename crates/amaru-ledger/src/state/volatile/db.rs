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

use amaru_kernel::{MemoizedTransactionOutput, Point, TransactionInput};

use crate::state::AnchoredVolatileFragment;

/// Volatile DB.
///
/// Anchored fragments are wrapped in `Arc` so that cloning the `VolatileDB`
/// is O(`k`) atomic refcount increments rather than a deep copy of every
/// fragment's diffs.
/// Fragments are immutable once anchored, so sharing them by `Arc` is sound: container-level operations
/// (`push_back`, `pop_front`, `rollback_to`) operate on the outer `VecDeque`, never on
/// the contents of an anchored fragment.
#[derive(Default, Clone)]
pub struct VolatileDB {
    sequence: VecDeque<Arc<AnchoredVolatileFragment>>,
}

impl VolatileDB {
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    pub fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.back().map(Arc::as_ref)
    }

    pub fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.front().map(Arc::as_ref)
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        for anchored in self.sequence.iter().rev() {
            if anchored.fragment.utxo.consumed.contains(input) {
                return None;
            }
            if let Some(output) = anchored.fragment.utxo.produced.get(input) {
                return Some(output);
            }
        }
        None
    }

    pub fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.sequence.iter().any(|anchored| anchored.fragment.utxo.consumed.contains(input))
    }

    pub fn contains(&self, point: &Point) -> bool {
        self.sequence.binary_search_by_key(point, |anchored| anchored.point()).is_ok()
    }

    /// Pop the oldest fragment off the front of the volatile DB.
    ///
    /// Panics if the popped `Arc<AnchoredVolatileFragment>` has more than one strong holder.
    /// The should only ever be one stage making modifications to the ledger so this
    /// invariant should always hold.
    #[expect(clippy::expect_used)]
    pub fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        self.sequence
            .pop_front()
            .map(|arc| Arc::try_unwrap(arc).expect("VolatileDB fragment had multiple Arc holders at pop_front"))
    }

    pub fn push_back(&mut self, fragment: AnchoredVolatileFragment) {
        self.sequence.push_back(Arc::new(fragment));
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
        Ok(())
    }

    pub fn clear(&mut self) {
        self.sequence.clear();
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &AnchoredVolatileFragment> {
        self.sequence.iter().map(Arc::as_ref)
    }
}

#[cfg(test)]
impl VolatileDB {
    pub fn fixture() -> Self {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1, 0));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2, 0));
        db.push_back(AnchoredVolatileFragment::fixture(30, 3, 0));
        assert_eq!(db.len(), 3);
        db
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Hash, Point, Slot};

    use super::*;

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
        let mut anchored = AnchoredVolatileFragment::fixture(10, 1, 0);
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
        let first = AnchoredVolatileFragment::fixture(10, 1, 0);
        let first_point = first.point();
        db.push_back(first);

        let mut second = AnchoredVolatileFragment::fixture(20, 2, 0);
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
}
