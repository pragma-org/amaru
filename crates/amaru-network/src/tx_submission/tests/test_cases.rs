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

use crate::tx_submission::Blocking;
use crate::tx_submission::Message::Init;
use crate::tx_submission::Outcome::{Error, Send};
use crate::tx_submission::ProtocolError::NoTxsRequested;
use crate::tx_submission::tests::faulty_tx_validator::FaultyTxValidator;
use crate::tx_submission::tests::test_data::{
    create_transactions, reply_tx_ids, reply_txs, request_tx_ids, request_txs,
};
use crate::tx_submission::tests::{
    Nodes, NodesOptions, SizedMempool, assert_outcomes_eq, expect_responder_transactions,
    expect_transactions,
};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros::TxSubmissionMempool;
use amaru_ouroboros_traits::can_validate_transactions::mock::MockCanValidateTransactions;
use std::sync::Arc;
// These tests cover the interaction between an initiator and a responder, with
// various initial conditions.

/// Test that all the transactions known to the initiator are eventually received by the responder.
#[tokio::test]
async fn test_initiator_responder_interaction() -> anyhow::Result<()> {
    let txs = create_transactions(8);
    let responder_options = NodesOptions::default()
        .with_initiator_mempool_capacity(8)
        .with_max_window(8)
        .with_fetch_batch(2);
    let mut nodes = Nodes::new(responder_options);
    nodes.insert_client_transactions(&txs);
    nodes.start().await?;

    expect_responder_transactions(&nodes, txs);
    Ok(())
}

/// Test that all the transactions known to the initiator are eventually received by the responder.
/// The transactions are added to the initiator mempool concurrently while the responder is fetching them.
#[tokio::test]
async fn test_with_concurrent_filling_of_the_client_mempool() -> anyhow::Result<()> {
    let txs = create_transactions(8);
    let initiator_mempool = Arc::new(SizedMempool::with_tx_validator(
        4,
        Arc::new(MockCanValidateTransactions),
    ));
    let responder_mempool = Arc::new(InMemoryMempool::default());

    let options = NodesOptions::default()
        .with_initiator_mempool(initiator_mempool.clone())
        .with_responder_mempool(responder_mempool.clone());
    let mut nodes = Nodes::new(options);

    let task1 = tokio::spawn(async move {
        nodes.start().await.unwrap();
    });

    let txs_clone = txs.clone();
    let initiator_mempool = initiator_mempool.clone();
    let task2 = tokio::spawn(async move {
        for tx in txs_clone {
            initiator_mempool.add(tx.clone()).unwrap();
        }
    });

    let (r1, r2) = tokio::join!(task1, task2);
    r1?;
    r2?;

    expect_transactions(responder_mempool, txs);
    Ok(())
}

/// Test that invalid transactions are not added to the responder mempool,
/// and that the responder does not request them again.
#[tokio::test]
async fn test_invalid_transactions() -> anyhow::Result<()> {
    let txs = create_transactions(6);

    // This validator rejects every second transaction
    let tx_validator = Arc::new(FaultyTxValidator::default());
    let responder_options = NodesOptions::default()
        .with_initiator_mempool_capacity(6)
        .with_responder_tx_validator(tx_validator)
        .with_max_window(6)
        .with_fetch_batch(2);

    let mut nodes = Nodes::new(responder_options);
    nodes.insert_client_transactions(&txs);
    nodes.start().await?;

    // Only the valid transactions (even indexed) should be in the responder mempool
    let mut expected = vec![];
    for (i, tx) in txs.iter().enumerate() {
        if i % 2 == 0 {
            expected.push(tx.clone());
        }
    }
    expect_responder_transactions(&nodes, expected);

    // Check the requests sent by the responder do not ask again for the invalid transactions.
    let actual = &nodes.outcomes;

    let expected = &[
        Send(Init),
        Send(request_tx_ids(0, 6, Blocking::Yes)),
        Send(reply_tx_ids(&txs, &[0, 1, 2, 3, 4, 5])),
        Send(request_txs(&txs, &[0, 1])),
        Send(reply_txs(&txs, &[0, 1])),
        Send(request_tx_ids(1, 1, Blocking::No)),
        Send(reply_tx_ids(&txs, &[])),
        Send(request_txs(&txs, &[2, 3])),
        Send(reply_txs(&txs, &[2, 3])),
        Send(request_tx_ids(0, 1, Blocking::No)),
        Send(reply_tx_ids(&txs, &[])),
        Send(request_txs(&txs, &[4, 5])),
        Send(reply_txs(&txs, &[4, 5])),
        Send(request_tx_ids(0, 1, Blocking::No)),
        Send(reply_tx_ids(&txs, &[])),
        Send(request_txs(&txs, &[])),
        Error(NoTxsRequested),
    ];
    assert_outcomes_eq(actual, expected);
    Ok(())
}
