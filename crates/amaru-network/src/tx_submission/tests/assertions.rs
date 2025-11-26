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

use crate::tx_submission::tests::{MessageEq, NodeHandle, Tx};
use amaru_ouroboros_traits::Mempool;
use pallas_network::miniprotocols::txsubmission::Message::{ReplyTxIds, ReplyTxs};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message, TxIdAndSize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::time::{sleep, timeout};

/// Check that all the given transactions are eventually present in the server mempool.
pub async fn expect_server_transactions(txs: Vec<Tx>, node_handle: &NodeHandle) {
    let server_mempool: Arc<dyn Mempool<Tx>> = node_handle.server_mempool.clone();
    let tx_ids: Vec<_> = txs.iter().map(|tx| tx.tx_id()).collect();

    timeout(Duration::from_secs(10), async {
        loop {
            let all_present = tx_ids.iter().all(|id| server_mempool.contains(id));
            if all_present {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all the transactions should have been transmitted to the server mempool");
}

/// Check that the next message is a ReplyTxIds with the expected ids.
pub async fn assert_tx_ids_reply(
    rx_messages: &mut Receiver<Message<EraTxId, EraTxBody>>,
    era_tx_ids: &[EraTxId],
    expected_ids: &[usize],
) -> anyhow::Result<()> {
    let tx_ids_and_sizes: Vec<TxIdAndSize<EraTxId>> = expected_ids
        .iter()
        .map(|&i| TxIdAndSize(era_tx_ids[i].clone(), 32))
        .collect();
    assert_next_message(rx_messages, ReplyTxIds(tx_ids_and_sizes)).await?;
    Ok(())
}

/// Check that the next message is a ReplyTxs with the expected transaction bodies.
pub async fn assert_tx_bodies_reply(
    rx_messages: &mut Receiver<Message<EraTxId, EraTxBody>>,
    era_tx_bodies: &[EraTxBody],
    expected_body_ids: &[usize],
) -> anyhow::Result<()> {
    let txs: Vec<EraTxBody> = expected_body_ids
        .iter()
        .map(|&i| era_tx_bodies[i].clone())
        .collect();
    assert_next_message(rx_messages, ReplyTxs(txs)).await?;
    Ok(())
}

/// Check that the next message matches the expected one.
pub async fn assert_next_message(
    rx_messages: &mut Receiver<Message<EraTxId, EraTxBody>>,
    expected: Message<EraTxId, EraTxBody>,
) -> anyhow::Result<()> {
    let actual = rx_messages
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("no message received"))?;
    let actual: MessageEq = actual.into();
    let expected: MessageEq = expected.into();
    assert_eq!(actual, expected, "actual = {actual}\nexpected = {expected}");
    Ok(())
}
