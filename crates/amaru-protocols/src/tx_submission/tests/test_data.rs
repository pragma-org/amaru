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

use amaru_kernel::{
    Hash, Transaction, TransactionBody, TransactionInput, WitnessSet, size::TRANSACTION_BODY,
};
use amaru_ouroboros_traits::Mempool;
use std::sync::Arc;

pub fn create_transactions(number: u64) -> Vec<Transaction> {
    (0..number).map(create_transaction).collect()
}

pub fn create_transactions_in_mempool(
    mempool: Arc<dyn Mempool<Transaction>>,
    number: u64,
) -> Vec<Transaction> {
    let mut txs = vec![];
    for i in 0..number {
        let tx = create_transaction(i);
        txs.push(tx.clone());
        mempool.add(tx).unwrap();
    }
    txs
}

/// Create a transaction with a unique input based on the given id.
pub fn create_transaction(id: u64) -> Transaction {
    let tx_input = TransactionInput {
        transaction_id: Hash::new([1; TRANSACTION_BODY]),
        index: id,
    };

    let body = TransactionBody::new([tx_input], [], 0);

    Transaction {
        body,
        witnesses: WitnessSet::default(),
        is_expected_valid: true,
        auxiliary_data: None,
    }
}
