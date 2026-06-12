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

mod db;
use amaru_kernel::{MemoizedTransactionOutput, Point, TransactionInput};
pub use db::VolatileDB;

mod fragment;
pub use fragment::{AnchoredVolatileFragment, StoreUpdate, VolatileFragment};

mod series;
pub use series::VolatileSeries;

mod view;
pub use view::VolatileView;

#[cfg(test)]
pub(crate) mod test_support;

pub trait VolatileStore {
    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
    fn view_back(&self) -> Option<&AnchoredVolatileFragment>;
    fn view_front(&self) -> Option<&AnchoredVolatileFragment>;
    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput>;
    fn has_consumed_input(&self, input: &TransactionInput) -> bool;
    fn contains(&self, point: &Point) -> bool;
    fn pop_front(&mut self) -> Option<AnchoredVolatileFragment>;
    fn push_back(&mut self, fragment: AnchoredVolatileFragment);
    fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point>;
    fn clear(&mut self);
    fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment>;
}
