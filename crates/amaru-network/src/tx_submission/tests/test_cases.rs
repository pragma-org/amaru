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

use crate::tx_submission::tests::{
    FaultyTxValidator, ServerOptions, Tx, create_node, create_node_with, expect_server_transactions,
};
use crate::tx_submission::{Blocking, new_era_tx_id};
use pallas_network::miniprotocols::txsubmission::Message::{RequestTxIds, RequestTxs};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message};
use std::sync::Arc;

#[tokio::test]
async fn test_client_server_interaction() -> anyhow::Result<()> {
    let txs = create_transactions(8);
    let server_options = ServerOptions::default()
        .with_mempool_capacity(8)
        .with_max_window(8)
        .with_fetch_batch(2)
        .with_blocking(Blocking::Yes);
    let node = create_node_with(server_options);
    node.insert_client_transactions(&txs);
    let node_handle = node.start();

    expect_server_transactions(txs, &node_handle).await;
    Ok(())
}

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

#[tokio::test]
async fn test_client_with_non_blocking_server() -> anyhow::Result<()> {
    let txs = create_transactions(8);
    let server_options = ServerOptions::default()
        .with_mempool_capacity(8)
        .with_max_window(8)
        .with_fetch_batch(2)
        .with_blocking(Blocking::No);
    let node = create_node_with(server_options);
    node.insert_client_transactions(&txs);
    let node_handle = node.start();

    expect_server_transactions(txs, &node_handle).await;
    Ok(())
}

#[tokio::test]
async fn test_invalid_transactions() -> anyhow::Result<()> {
    let txs = create_transactions(6);
    let era_tx_ids: Vec<EraTxId> = txs.iter().map(|tx| new_era_tx_id(tx.tx_id())).collect();

    // This validator rejects every second transaction
    let tx_validator = Arc::new(FaultyTxValidator::default());
    let server_options = ServerOptions::default()
        .with_mempool_capacity(6)
        .with_max_window(6)
        .with_fetch_batch(2)
        .with_blocking(Blocking::Yes)
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
        RequestTxIds(true, 2, 2),
        RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
        RequestTxIds(true, 2, 4),
        RequestTxs(vec![era_tx_ids[4].clone(), era_tx_ids[5].clone()]),
        RequestTxIds(true, 2, 6),
    ];
    assert_eq!(actual, expected);
    Ok(())
}

// HELPERS

pub fn create_transactions(number: u16) -> Vec<Tx> {
    let mut txs = vec![];
    for i in 0..number {
        txs.push(Tx::new(format!("{i}d8d00cdd4657ac84d82f0a56067634a")));
    }
    txs
}
