// Copyright 2025 PRAGMA
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

use crate::{IntoKeys, Mempool};
use std::{collections::BTreeSet, mem, ops::Deref};

#[derive(Debug, Default)]
pub struct DummyMempool<T> {
    transactions: Vec<T>,
}

impl<T> DummyMempool<T> {
    pub fn new() -> Self {
        DummyMempool {
            transactions: vec![],
        }
    }
}

impl<T> Mempool<T> for DummyMempool<T>
where
    T: Deref + Send + Sync,
    T::Target: IntoKeys,
    <T::Target as IntoKeys>::Key: Ord,
{
    fn add(&mut self, tx: T) {
        self.transactions.push(tx);
    }

    fn take(&mut self) -> Vec<T> {
        mem::take(&mut self.transactions)
    }

    fn acknowledge(&mut self, tx: &T::Target) {
        let refs: BTreeSet<&<T::Target as IntoKeys>::Key> = tx.keys().collect();
        self.transactions
            .retain(|tx| !tx.keys().any(|input| refs.contains(input)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct FakeTx<'a> {
        id: &'a str,
        inputs: Vec<usize>,
    }

    impl<'a> FakeTx<'a> {
        fn new(id: &'a str, inputs: &'_ [usize]) -> Self {
            FakeTx {
                id,
                inputs: Vec::from(inputs),
            }
        }
    }

    impl Deref for FakeTx<'_> {
        type Target = Self;

        fn deref(&self) -> &Self::Target {
            self
        }
    }

    impl IntoKeys for FakeTx<'_> {
        type Key = usize;

        fn keys(&self) -> impl Iterator<Item = &Self::Key> {
            self.inputs.iter()
        }
    }

    #[test]
    fn take_empty() {
        let mut mempool: DummyMempool<FakeTx<'_>> = DummyMempool::new();
        assert_eq!(mempool.take(), vec![]);
    }

    #[test]
    fn add_then_take() {
        let mut mempool = DummyMempool::new();
        mempool.add(FakeTx::new("tx1", &[1]));
        mempool.add(FakeTx::new("tx2", &[2]));
        assert_eq!(
            mempool.take(),
            vec![FakeTx::new("tx1", &[1]), FakeTx::new("tx2", &[2])]
        );
    }

    #[test]
    fn invalidate_entries() {
        let mut mempool = DummyMempool::new();
        mempool.add(FakeTx::new("tx1", &[1, 2]));
        mempool.add(FakeTx::new("tx2", &[3, 4]));
        mempool.add(FakeTx::new("tx3", &[5, 6]));
        mempool.acknowledge(&FakeTx::new("tx4", &[2, 5, 7]));
        assert_eq!(mempool.take(), vec![FakeTx::new("tx2", &[3, 4])]);
    }
}
