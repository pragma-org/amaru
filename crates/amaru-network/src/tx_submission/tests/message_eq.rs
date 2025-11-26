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
use crate::tx_submission::tests::test_cases::{era_tx_body_to_string, era_tx_id_to_string};
use pallas_network::miniprotocols::txsubmission::Message::*;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message, TxIdAndSize};
use std::fmt::{Debug, Display};

/// Wrapper around Message to implement custom Display, Debug and PartialEq
pub struct MessageEq(Message<EraTxId, EraTxBody>);

impl MessageEq {
    pub fn new(message: Message<EraTxId, EraTxBody>) -> Self {
        MessageEq(message)
    }
}

impl From<MessageEq> for Message<EraTxId, EraTxBody> {
    fn from(m: MessageEq) -> Self {
        m.0
    }
}

impl From<Message<EraTxId, EraTxBody>> for MessageEq {
    fn from(value: Message<EraTxId, EraTxBody>) -> Self {
        MessageEq::new(value)
    }
}

impl Clone for MessageEq {
    fn clone(&self) -> Self {
        MessageEq::from(&self.0)
    }
}

impl From<&Message<EraTxId, EraTxBody>> for MessageEq {
    fn from(m: &Message<EraTxId, EraTxBody>) -> Self {
        let clone = match &m {
            Init => Init,
            RequestTxIds(blocking, tx_count, tx_count2) => {
                RequestTxIds(*blocking, *tx_count, *tx_count2)
            }
            RequestTxs(tx_ids) => RequestTxs(tx_ids.to_vec()),
            ReplyTxIds(tx_ids_and_sizes) => ReplyTxIds(
                tx_ids_and_sizes
                    .iter()
                    .map(|t| TxIdAndSize(t.0.clone(), t.1))
                    .collect(),
            ),
            ReplyTxs(tx_bodies) => ReplyTxs(tx_bodies.to_vec()),
            Done => Done,
        };
        MessageEq::new(clone)
    }
}

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
