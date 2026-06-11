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

use amaru_kernel::{MemoizedTransactionOutput, Point, TransactionInput};

use crate::state::{
    AnchoredVolatileFragment,
    overlay::StateOverlay,
    volatile::{VolatileSeries, VolatileStore},
};

#[derive(Default)]
pub struct VolatileDB {
    current: VolatileSeries,
    draining: Option<(VolatileSeries, StateOverlay)>,
}

impl VolatileStore for VolatileDB {
    fn is_empty(&self) -> bool {
        self.current.is_empty() && self.draining.as_ref().is_none_or(|(draining, _)| draining.is_empty())
    }

    fn len(&self) -> usize {
        self.current.len() + self.draining.as_ref().map(|(draining, _)| draining.len()).unwrap_or_default()
    }

    fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.current.view_back().or(self.draining.as_ref().and_then(|(draining, _)| draining.view_back()))
    }

    fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.draining.as_ref().and_then(|(draining, _)| draining.view_front()).or(self.current.view_front())
    }

    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        if self.has_consumed_input(input) {
            return None;
        }

        self.current
            .resolve_input(input)
            .or(self.draining.as_ref().and_then(|(draining, _)| draining.resolve_input(input)))
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.current.has_consumed_input(input)
            || self.draining.as_ref().is_some_and(|(draining, _)| draining.has_consumed_input(input))
    }

    fn contains(&self, point: &Point) -> bool {
        self.current.contains(point) || self.draining.as_ref().is_some_and(|(draining, _)| draining.contains(point))
    }

    fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        self.draining.as_mut().and_then(|(draining, _)| draining.pop_front()).or(self.current.pop_front())
    }

    fn push_back(&mut self, fragment: AnchoredVolatileFragment) {
        // By design, we should never be pushing to the back of the draining sequence
        self.current.push_back(fragment);
    }

    #[allow(clippy::expect_used)]
    fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point> {
        let target_slot = point.slot_or_default();

        // Check if the target point is beyond the current sequence
        // In this case we simply return Ok since it this would not change the volatile DB
        if let Some(last) = self.current.view_back()
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

        // Check if the target point is before the active sequence
        // In this case we return an error since it means rolling back the stable DB
        if let Some(first) =
            self.draining.as_ref().and_then(|(draining, _)| draining.view_front()).or(self.current.view_front())
            && target_slot < first.slot()
        {
            tracing::error!(
                name: "rollback_to.before",
                %target_slot,
                first_slot = ?first.slot(),
                "Attempting to rollback to a point before the first known of the volatile fragment"
            );
            return Err(point);
        }

        // Now we know the target point is within our volatile DB.
        // Keep all elements with point <= target point.

        if self.current.contains(point) {
            self.current.rollback_to(point)?;
            return Ok(());
        }

        if self.draining.as_ref().is_some_and(|(draining, _)| draining.contains(point)) {
            // If we are rolling back to a point in the draining sequence, we need to
            // promote the remaining sequence (after truncating) to current.
            let (mut draining, _) = self.draining.take().expect("draining is Some");

            draining.rollback_to(point)?;

            self.current = draining;

            return Ok(());
        }

        Err(point)
    }

    fn clear(&mut self) {
        self.current.clear();
        self.draining = None;
    }

    fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment> {
        self.draining.as_ref().map(|(d, _)| d.iter()).into_iter().flatten().chain(self.current.iter())
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
}
