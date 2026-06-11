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

use std::collections::VecDeque;

use amaru_kernel::{MemoizedTransactionOutput, Point, TransactionInput};

use crate::state::{AnchoredVolatileFragment, VolatileFragment, volatile::VolatileStore};

// TODO: Maintain full aggregate
//
// Currently, recompute aggregate is only the UTxOs, and we never even call it!
// Our aggregate sits empty for now
#[derive(Default)]
pub struct VolatileSeries {
    sequence: VecDeque<AnchoredVolatileFragment>,
    aggregate: VolatileFragment,
}

impl VolatileStore for VolatileSeries {
    fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    fn len(&self) -> usize {
        self.sequence.len()
    }

    fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.back()
    }

    fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.front()
    }

    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.aggregate.utxo.produced.get(input)
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.aggregate.utxo.consumed.contains(input)
    }

    fn contains(&self, point: &Point) -> bool {
        self.sequence.binary_search_by_key(point, |anchored| anchored.point()).is_ok()
    }

    fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        self.sequence.pop_front()
    }

    fn push_back(&mut self, fragment: AnchoredVolatileFragment) {
        self.sequence.push_back(fragment);
    }

    fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point> {
        let ix = self.sequence.binary_search_by_key(point, |anchored| anchored.point()).map_err(|_| point)?;

        self.sequence.truncate(ix + 1);

        self.recompute_aggregate();
        Ok(())
    }

    fn clear(&mut self) {
        self.sequence.clear();
        self.aggregate = Default::default();
    }

    fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment> {
        self.sequence.iter()
    }
}

impl VolatileSeries {
    fn recompute_aggregate(&mut self) {
        let mut aggregate = VolatileFragment::default();
        for anchored in &self.sequence {
            // This clone should go away once we use reference counters
            aggregate.utxo.merge(anchored.fragment.utxo.clone());
        }

        self.aggregate = aggregate;
    }
}
