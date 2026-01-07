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
use amaru_kernel::{Tx, bytes::NonEmptyBytes};
use amaru_ouroboros_traits::TxId;
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

/// Messages for the txsubmission mini-protocol.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum Message {
    Init,
    RequestTxIdsBlocking(u16, u16),
    RequestTxIdsNonBlocking(u16, u16),
    RequestTxs(Vec<TxId>),
    ReplyTxIds(Vec<(TxId, u32)>),
    ReplyTxs(Vec<Tx>),
    Done,
}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let serialized_self = minicbor::to_vec(self).unwrap_or_default();
        let serialized_other = minicbor::to_vec(other).unwrap_or_default();
        serialized_self.cmp(&serialized_other)
    }
}

impl Message {
    pub fn message_type(&self) -> &'static str {
        match self {
            Message::Init => "Init",
            Message::RequestTxIdsBlocking(_, _) => "RequestTxIdsBlocking",
            Message::RequestTxIdsNonBlocking(_, _) => "RequestTxIdsNonBlocking",
            Message::ReplyTxIds(_) => "ReplyTxIds",
            Message::RequestTxs(_) => "RequestTxs",
            Message::ReplyTxs(_) => "ReplyTxs",
            Message::Done => "Done",
        }
    }
}

impl Encode<()> for Message {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::RequestTxIdsBlocking(ack, req) => {
                e.array(4)?.u16(0)?;
                e.encode(Blocking::Yes)?;
                e.u16(*ack)?;
                e.u16(*req)?;
            }
            Message::RequestTxIdsNonBlocking(ack, req) => {
                e.array(4)?.u16(0)?;
                e.encode(Blocking::No)?;
                e.u16(*ack)?;
                e.u16(*req)?;
            }
            Message::ReplyTxIds(ids) => {
                e.array(2)?.u16(1)?;
                e.begin_array()?;
                for id in ids {
                    e.encode(id)?;
                }
                e.end()?;
            }
            Message::RequestTxs(ids) => {
                e.array(2)?.u16(2)?;
                e.begin_array()?;
                for id in ids {
                    e.encode(id)?;
                }
                e.end()?;
            }
            Message::ReplyTxs(txs) => {
                e.array(2)?.u16(3)?;
                e.begin_array()?;
                for tx in txs {
                    e.encode(tx)?;
                }
                e.end()?;
            }
            Message::Done => {
                e.array(1)?.u16(4)?;
            }
            Message::Init => {
                e.array(1)?.u16(6)?;
            }
        }
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for Message {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        d.array()?;
        let label = d.u16()?;

        match label {
            0 => {
                let blocking = d.decode()?;
                let ack = d.u16()?;
                let req = d.u16()?;
                match blocking {
                    Blocking::Yes => Ok(Message::RequestTxIdsBlocking(ack, req)),
                    Blocking::No => Ok(Message::RequestTxIdsNonBlocking(ack, req)),
                }
            }
            1 => {
                let items = d.decode()?;
                Ok(Message::ReplyTxIds(items))
            }
            2 => {
                let ids = d.decode()?;
                Ok(Message::RequestTxs(ids))
            }
            3 => Ok(Message::ReplyTxs(
                d.array_iter()?.collect::<Result<_, _>>()?,
            )),
            4 => Ok(Message::Done),
            6 => Ok(Message::Init),
            _ => Err(decode::Error::message(
                "unknown variant for txsubmission message",
            )),
        }
    }
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::Init => write!(f, "Init"),
            Message::RequestTxIdsBlocking(ack, req) => {
                write!(f, "RequestTxIdsBlocking(ack: {}, req: {})", ack, req,)
            }
            Message::RequestTxIdsNonBlocking(ack, req) => {
                write!(f, "RequestTxIdsNonBlocking(ack: {}, req: {})", ack, req,)
            }
            Message::ReplyTxIds(ids) => {
                write!(
                    f,
                    "ReplyTxIds(ids: [{}])",
                    ids.iter()
                        .map(|(id, size)| format!("({}, {})", id, size))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Message::RequestTxs(ids) => {
                write!(
                    f,
                    "RequestTxs(ids: [{}])",
                    ids.iter()
                        .map(|id| format!("{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Message::ReplyTxs(txs) => {
                write!(
                    f,
                    "ReplyTxs(txs: [{}])",
                    txs.iter()
                        .map(|tx| format!("{}", TxId::from(tx)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Message::Done => write!(f, "Done"),
        }
    }
}

/// Messages coming directly from the muxer.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum TxSubmissionMessage {
    Registered,
    FromNetwork(NonEmptyBytes),
}

/// Roundtrip property tests for txsubmission messages.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx_submission::tests::create_transaction;
    use amaru_kernel::{Hash, prop_cbor_roundtrip};
    use pallas_primitives::conway::Tx;
    use prop::collection::vec;
    use proptest::prelude::*;
    use proptest::prop_compose;

    mod tx_id {
        use super::*;
        prop_cbor_roundtrip!(TxId, any_tx_id());
    }
    mod message {
        use super::*;
        prop_cbor_roundtrip!(Message, any_message());
    }
    mod blocking {
        use super::*;
        prop_cbor_roundtrip!(Blocking, any_blocking());
    }

    // HELPERS

    prop_compose! {
        pub fn any_tx_id()(
            bytes in any::<[u8; 32]>(),
        ) -> TxId {
            TxId::new(Hash::new(bytes))
        }
    }

    prop_compose! {
        pub fn any_blocking()(
            bool in any::<bool>(),
        ) -> Blocking {
            if bool { Blocking::Yes } else { Blocking::No }
        }
    }

    prop_compose! {
        fn any_ack_req()(ack in 0u16..=1000, req in 0u16..=1000) -> (u16, u16) {
            (ack, req)
        }
    }

    prop_compose! {
        fn any_tx_id_and_sizes_vec()(ids in vec(any_tx_id(), 0..20), sizes in vec(any::<u32>(), 0..20)) -> Vec<(TxId, u32)> {
            ids.iter().zip(sizes).map(|(id, size)| (*id, size)).collect()
        }
    }

    prop_compose! {
        fn any_tx_id_vec()(ids in prop::collection::vec(any_tx_id(), 0..20)) -> Vec<TxId> {
            ids
        }
    }

    prop_compose! {
        fn any_tx_vec()(txs in prop::collection::vec(any_tx(), 0..10)) -> Vec<Tx> {
            txs
        }
    }

    prop_compose! {
        fn any_tx()(n in 0u64..=1000) -> Tx {
            create_transaction(n)
        }
    }

    fn init_message() -> impl Strategy<Value = Message> {
        Just(Message::Init)
    }

    prop_compose! {
        fn request_tx_ids_message()((ack, req) in any_ack_req(), blocking in any_blocking()) -> Message {
            match blocking {
                Blocking::Yes => Message::RequestTxIdsBlocking(ack, req),
                Blocking::No => Message::RequestTxIdsNonBlocking(ack, req),
            }
        }
    }

    prop_compose! {
        fn reply_tx_ids_message()(ids in any_tx_id_and_sizes_vec()) -> Message {
            Message::ReplyTxIds(ids)
        }
    }

    prop_compose! {
        fn request_txs_message()(ids in any_tx_id_vec()) -> Message {
            Message::RequestTxs(ids)
        }
    }

    prop_compose! {
        fn reply_txs_message()(txs in any_tx_vec()) -> Message {
            Message::ReplyTxs(txs)
        }
    }

    fn done_message() -> impl Strategy<Value = Message> {
        Just(Message::Done)
    }

    pub fn any_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            1 => init_message(),
            3 => request_tx_ids_message(),
            3 => reply_tx_ids_message(),
            3 => request_txs_message(),
            3 => reply_txs_message(),
            1 => done_message(),
        ]
    }
}
