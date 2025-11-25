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

use crate::tx_submission::EraTxIdOrd;
use amaru_kernel::Hash;
use amaru_ouroboros_traits::TxId;
use minicbor::encode::{Error, Write};
use minicbor::{CborLen, Decode, Decoder, Encode, Encoder};
use pallas_network::miniprotocols::txsubmission::Message::{ReplyTxIds, ReplyTxs};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message, TxIdAndSize};
use std::fmt::{Debug, Display};
use tokio::sync::mpsc::Receiver;

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
    let actual = MessageEq(actual);
    let expected = MessageEq(expected);
    assert_eq!(actual, expected, "actual = {actual}\nexpected = {expected}");
    Ok(())
}

/// Simple transaction data type for tests.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Tx {
    tx_body: String,
}

impl Tx {
    pub fn new(tx_body: impl Into<String>) -> Self {
        Self {
            tx_body: tx_body.into(),
        }
    }

    pub fn tx_id(&self) -> TxId {
        TxId::from(self.tx_body.as_str())
    }

    pub fn tx_body(&self) -> Vec<u8> {
        minicbor::to_vec(&self.tx_body).unwrap()
    }
}

impl CborLen<()> for Tx {
    fn cbor_len(&self, _ctx: &mut ()) -> usize {
        self.tx_body.len()
    }
}

impl Encode<()> for Tx {
    fn encode<W: Write>(&self, e: &mut Encoder<W>, _ctx: &mut ()) -> Result<(), Error<W::Error>> {
        e.encode(&self.tx_body)?;
        Ok(())
    }
}

impl<'a> Decode<'a, ()> for Tx {
    fn decode(d: &mut Decoder<'a>, _ctx: &mut ()) -> Result<Self, minicbor::decode::Error> {
        let tx_body: String = d.decode()?;
        Ok(Tx { tx_body })
    }
}

/// Wrapper around Message to implement custom Display, Debug and PartialEq
pub struct MessageEq(Message<EraTxId, EraTxBody>);

impl Debug for MessageEq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl Display for MessageEq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Message::Init => write!(f, "Init"),
            Message::RequestTxIds(blocking, ack, req) => {
                write!(
                    f,
                    "RequestTxIds(blocking={}, ack={}, req={})",
                    blocking, ack, req
                )
            }
            ReplyTxIds(ids_and_sizes) => {
                let ids: Vec<String> = ids_and_sizes
                    .iter()
                    .map(|tx_id_and_size| era_tx_id_to_string(&tx_id_and_size.0))
                    .collect();
                write!(f, "ReplyTxIds(ids=[{}])", ids.join(", "))
            }
            Message::RequestTxs(ids) => {
                let ids_str: Vec<String> = ids.iter().map(era_tx_id_to_string).collect();
                write!(f, "RequestTxs(ids=[{}])", ids_str.join(", "))
            }
            ReplyTxs(bodies) => {
                let bodies_str: Vec<String> = bodies.iter().map(era_tx_body_to_string).collect();
                write!(f, "ReplyTxs(bodies=[{}])", bodies_str.join(", "))
            }
            Message::Done => write!(f, "Done"),
        }
    }
}

impl PartialEq for MessageEq {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Message::Done, Message::Done) => true,
            (Message::Init, Message::Init) => true,
            (Message::RequestTxIds(b1, a1, r1), Message::RequestTxIds(b2, a2, r2)) => {
                b1 == b2 && a1 == a2 && r1 == r2
            }
            (Message::RequestTxs(ids1), Message::RequestTxs(ids2)) => {
                ids1.iter()
                    .map(|id| EraTxIdOrd::new(id.clone()))
                    .collect::<Vec<_>>()
                    == ids2
                        .iter()
                        .map(|id| EraTxIdOrd::new(id.clone()))
                        .collect::<Vec<_>>()
            }
            (ReplyTxIds(ids1), ReplyTxIds(ids2)) => {
                ids1.iter()
                    .map(|id| (id.0.0, id.0.1.clone(), id.1))
                    .collect::<Vec<_>>()
                    == ids2
                        .iter()
                        .map(|id| (id.0.0, id.0.1.clone(), id.1))
                        .collect::<Vec<_>>()
            }
            (ReplyTxs(txs1), ReplyTxs(txs2)) => {
                txs1.iter()
                    .cloned()
                    .map(|tx| (tx.0, tx.1))
                    .collect::<Vec<_>>()
                    == txs2
                        .iter()
                        .cloned()
                        .map(|tx| (tx.0, tx.1))
                        .collect::<Vec<_>>()
            }
            _ => false,
        }
    }
}

fn era_tx_id_to_string(era_tx_id: &EraTxId) -> String {
    Hash::<32>::from(era_tx_id.1.as_slice()).to_string()
}

fn era_tx_body_to_string(era_tx_body: &EraTxBody) -> String {
    String::from_utf8(era_tx_body.1.clone()).unwrap()
}
