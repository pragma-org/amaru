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

use crate::peer::Peer;
use itertools::Itertools;
use minicbor::Encode;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::conway::Tx;
use std::fmt;
use std::fmt::{Display, Formatter};
use tracing::Span;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TxRequest {
    Txs {
        peer: Peer,
        tx_ids: Vec<TxId>,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    TxIds {
        peer: Peer,
        ack: u16,
        req: u16,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    TxIdsNonBlocking {
        peer: Peer,
        ack: u16,
        req: u16,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl fmt::Debug for TxRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxRequest::Txs { peer, tx_ids, .. } => f
                .debug_struct("Txs")
                .field("peer", &peer.name)
                .field(
                    "tx_ids",
                    &tx_ids.iter().map(|tx_id| tx_id.to_string()).join(", "),
                )
                .finish(),
            TxRequest::TxIds { peer, ack, req, .. } => f
                .debug_struct("TxIds")
                .field("peer", &peer.name)
                .field("ack", &ack)
                .field("req", &req)
                .finish(),
            TxRequest::TxIdsNonBlocking { peer, ack, req, .. } => f
                .debug_struct("TxIdsNonBlocking")
                .field("peer", &peer.name)
                .field("ack", &ack)
                .field("req", &req)
                .finish(),
        }
    }
}

impl TxRequest {
    pub fn set_span(&mut self, span: Span) {
        match self {
            TxRequest::Txs { span: s, .. } => {
                *s = span;
            }
            TxRequest::TxIds { span: s, .. } => {
                *s = span;
            }
            TxRequest::TxIdsNonBlocking { span: s, .. } => {
                *s = span;
            }
        }
    }

    pub fn span(&self) -> &Span {
        match self {
            TxRequest::Txs { span, .. } => span,
            TxRequest::TxIds { span, .. } => span,
            TxRequest::TxIdsNonBlocking { span, .. } => span,
        }
    }

    pub fn peer(&self) -> &Peer {
        match self {
            TxRequest::Txs { peer, .. } => peer,
            TxRequest::TxIds { peer, .. } => peer,
            TxRequest::TxIdsNonBlocking { peer, .. } => peer,
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TxReply {
    TxIds {
        peer: Peer,
        tx_ids: Vec<(TxId, u32)>,
    },
    Txs {
        peer: Peer,
        txs: Vec<Tx>,
    },
}

impl TxReply {
    pub fn peer(&self) -> &Peer {
        match self {
            TxReply::TxIds { peer, .. } => peer,
            TxReply::Txs { peer, .. } => peer,
        }
    }
}

impl fmt::Debug for TxReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxReply::TxIds { peer, tx_ids } => f
                .debug_struct("TxIds")
                .field("peer", peer)
                .field(
                    "tx_ids",
                    &tx_ids
                        .iter()
                        .map(|(tx_id, size)| format!("{} (size: {}))", tx_id, size))
                        .join(", "),
                )
                .finish(),
            TxReply::Txs { peer, txs } => f
                .debug_struct("Txs")
                .field("peer", peer)
                .field("tx_count", &txs.len())
                .finish(),
        }
    }
}

/// Identifier for a transaction in the mempool.
/// It is derived from the hash of the encoding of the transaction as CBOR.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct TxId(Hash<32>);

impl TxId {
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.as_ref().to_vec()
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl TxId {
    pub fn new(hash: Hash<32>) -> Self {
        TxId(hash)
    }

    pub fn from<Tx: Encode<()>>(tx: Tx) -> Self {
        TxId(Hasher::<{ 32 * 8 }>::hash_cbor(&tx))
    }
}

impl Display for TxId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.as_slice()))
    }
}
