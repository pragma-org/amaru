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

use crate::consensus::tx_submission::tests::{
    FaultyTxValidator, ServerOptions, create_node, create_node_with, expect_server_transactions,
};
use amaru_kernel::{Nullable, Set};
use amaru_network::era_tx_ids;
use amaru_ouroboros_traits::TxId;
use pallas_crypto::hash::Hash;
use pallas_network::miniprotocols::txsubmission::Message::{RequestTxIds, RequestTxs};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message};
use pallas_primitives::TransactionInput;
use pallas_primitives::conway::{PseudoTransactionBody, PseudoTx, Tx, WitnessSet};
use std::sync::Arc;

/// Test that all the transactions known to the client are eventually received by the server.
#[tokio::test]
async fn test_client_server_interaction() -> anyhow::Result<()> {
    let txs = create_transactions(8);
    let server_options = ServerOptions::default()
        .with_mempool_capacity(8)
        .with_max_window(8)
        .with_fetch_batch(2);
    let node = create_node_with(server_options);
    node.insert_client_transactions(&txs);
    let node_handle = node.start();

    expect_server_transactions(txs, &node_handle).await;
    Ok(())
}

/// Test that all the transactions known to the client are eventually received by the server.
/// The transactions are added to the client mempool concurrently while the server is fetching them.
#[tokio::test]
async fn test_client_server_with_concurrent_filling_of_the_client_mempool() -> anyhow::Result<()> {
    let txs = create_transactions(8);
    let node = create_node();

    let node_handle = node.start();
    let txs_clone = txs.clone();
    let client_mempool = node_handle.client_mempool.clone();
    tokio::spawn(async move {
        for tx in txs_clone {
            client_mempool.add(tx.clone()).unwrap();
        }
    });

    expect_server_transactions(txs, &node_handle).await;
    Ok(())
}

/// Test that invalid transactions are not added to the server mempool,
/// and that the server does not request them again.
#[tokio::test]
async fn test_invalid_transactions() -> anyhow::Result<()> {
    let txs = create_transactions(6);
    let tx_ids: Vec<TxId> = txs.iter().map(TxId::from).collect();
    let era_tx_ids = era_tx_ids(&tx_ids);

    // This validator rejects every second transaction
    let tx_validator = Arc::new(FaultyTxValidator::default());
    let server_options = ServerOptions::default()
        .with_mempool_capacity(6)
        .with_max_window(6)
        .with_fetch_batch(2)
        .with_tx_validator(tx_validator);
    let node = create_node_with(server_options);
    node.insert_client_transactions(&txs);
    let mut node_handle = node.start();

    // Only the valid transactions (even indexed) should be in the server mempool
    let mut expected = vec![];
    for (i, tx) in txs.iter().enumerate() {
        if i % 2 == 0 {
            expected.push(tx.clone());
        }
    }
    expect_server_transactions(expected, &node_handle).await;

    // Check the requests sent by the server do not ask again for the invalid transactions.
    let actual: Vec<Message<EraTxId, EraTxBody>> = node_handle.observe_messages().await;

    let expected = [
        RequestTxIds(true, 0, 6),
        RequestTxs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
        RequestTxIds(false, 2, 2),
        RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
        RequestTxIds(false, 2, 4),
        RequestTxs(vec![era_tx_ids[4].clone(), era_tx_ids[5].clone()]),
        RequestTxIds(true, 2, 6),
    ];
    assert_eq!(actual, expected);
    Ok(())
}

// HELPERS

pub fn create_transactions(number: u64) -> Vec<Tx> {
    let mut txs = vec![];
    for i in 0..number {
        txs.push(create_transaction(i));
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
