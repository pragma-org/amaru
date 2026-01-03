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

use crate::tx_submission::Message::{ReplyTxIds, ReplyTxs, RequestTxIds, RequestTxs};
use crate::tx_submission::{Blocking, Message};
use amaru_kernel::{Hash, Nullable, Set};
use amaru_ouroboros_traits::{Mempool, TxId};
use pallas_primitives::TransactionInput;
use pallas_primitives::conway::{PseudoTransactionBody, PseudoTx, Tx, WitnessSet};
use std::sync::Arc;

pub fn reply_tx_ids(txs: &[Tx], ids: &[usize]) -> Message {
    ReplyTxIds(ids.iter().map(|id| (TxId::from(&txs[*id]), 50)).collect())
}

pub fn reply_txs(txs: &[Tx], ids: &[usize]) -> Message {
    ReplyTxs(ids.iter().map(|id| txs[*id].clone()).collect())
}

pub fn request_tx_ids(ack: u16, req: u16, blocking: Blocking) -> Message {
    RequestTxIds(ack, req, blocking)
}

pub fn request_txs(txs: &[Tx], ids: &[usize]) -> Message {
    RequestTxs(ids.iter().map(|id| TxId::from(&txs[*id])).collect())
}

pub fn create_transactions(number: u64) -> Vec<Tx> {
    (0..number).map(create_transaction).collect()
}

pub fn create_transactions_in_mempool(mempool: Arc<dyn Mempool<Tx>>, number: u64) -> Vec<Tx> {
    let mut txs = vec![];
    for i in 0..number {
        let tx = create_transaction(i);
        txs.push(tx.clone());
        mempool.add(tx).unwrap();
    }
    txs
}

/// Create a transaction with a unique input based on the given id.
pub fn create_transaction(id: u64) -> Tx {
    let tx_input = TransactionInput {
        transaction_id: Hash::new([1; 32]),
        index: id,
    };
    let transaction_body = PseudoTransactionBody {
        inputs: Set::from(vec![tx_input]),
        outputs: vec![],
        fee: 0,
        ttl: None,
        certificates: None,
        withdrawals: None,
        auxiliary_data_hash: None,
        validity_interval_start: None,
        mint: None,
        script_data_hash: None,
        required_signers: None,
        network_id: None,
        collateral_return: None,
        total_collateral: None,
        reference_inputs: None,
        voting_procedures: None,
        proposal_procedures: None,
        treasury_value: None,
        collateral: None,
        donation: None,
    };
    let transaction_witness_set = WitnessSet {
        vkeywitness: None,
        native_script: None,
        bootstrap_witness: None,
        plutus_v1_script: None,
        plutus_data: None,
        redeemer: None,
        plutus_v2_script: None,
        plutus_v3_script: None,
    };
    PseudoTx {
        transaction_body,
        transaction_witness_set,
        success: true,
        auxiliary_data: Nullable::Null,
    }
}
