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

use amaru_kernel::Tx;
use amaru_ouroboros_traits::{TxId, TxSubmissionMempool};
use std::sync::Arc;

/// Check that all the given transactions are currently present in the given mempool.
#[track_caller]
pub fn expect_transactions(mempool: Arc<dyn TxSubmissionMempool<Tx>>, txs: Vec<Tx>) {
    let tx_ids: Vec<_> = txs.iter().map(TxId::from).collect();
    let actual = tx_ids
        .iter()
        .filter(|tx_id| mempool.contains(tx_id))
        .collect::<Vec<_>>();

    if actual.len() != txs.len() {
        panic!(
            "\nActual transactions\n\n{}\n\nExpected transactions\n\n{}\n",
            actual
                .iter()
                .map(|id| format!("{}", id))
                .collect::<Vec<_>>()
                .join(",\n"),
            tx_ids
                .iter()
                .map(|id| format!("{}", id))
                .collect::<Vec<_>>()
                .join(",\n")
        );
    }
}

/// Check a list of actions against the expected ones.
#[track_caller]
pub fn assert_actions_eq<T: std::fmt::Debug + std::fmt::Display + PartialEq>(
    actual: &[T],
    expected: &[T],
) {
    if actual != expected {
        panic!(
            "\nActual outcomes\n\n{}\n\nExpected outcomes\n\n{}\n",
            actual
                .iter()
                .map(|id| format!("{}", id))
                .collect::<Vec<_>>()
                .join(",\n"),
            expected
                .iter()
                .map(|id| format!("{}", id))
                .collect::<Vec<_>>()
                .join(",\n")
        );
    }
}
