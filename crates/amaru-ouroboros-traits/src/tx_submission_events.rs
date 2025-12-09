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

use amaru_kernel::peer::Peer;
use itertools::Itertools;
use minicbor::Encode;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::conway::Tx;
use std::fmt;
use std::fmt::{Display, Formatter};
use tracing::Span;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum TxServerRequest {
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

impl PartialEq for TxServerRequest {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                TxServerRequest::Txs {
                    peer: p1,
                    tx_ids: t1,
                    ..
                },
                TxServerRequest::Txs {
                    peer: p2,
                    tx_ids: t2,
                    ..
                },
            ) => p1 == p2 && t1 == t2,
            (
                TxServerRequest::TxIds {
                    peer: p1,
                    ack: a1,
                    req: r1,
                    ..
                },
                TxServerRequest::TxIds {
                    peer: p2,
                    ack: a2,
                    req: r2,
                    ..
                },
            ) => p1 == p2 && a1 == a2 && r1 == r2,
            (
                TxServerRequest::TxIdsNonBlocking {
                    peer: p1,
                    ack: a1,
                    req: r1,
                    ..
                },
                TxServerRequest::TxIdsNonBlocking {
                    peer: p2,
                    ack: a2,
                    req: r2,
                    ..
                },
            ) => p1 == p2 && a1 == a2 && r1 == r2,
            _ => false,
        }
    }
}

impl Eq for TxServerRequest {}

impl fmt::Debug for TxServerRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TxServerRequest::Txs { peer, tx_ids, .. } => f
                .debug_struct("Txs")
                .field("peer", &peer.name)
                .field(
                    "tx_ids",
                    &tx_ids.iter().map(|tx_id| tx_id.to_string()).join(", "),
                )
                .finish(),
            TxServerRequest::TxIds { peer, ack, req, .. } => f
                .debug_struct("TxIds")
                .field("peer", &peer.name)
                .field("ack", &ack)
                .field("req", &req)
                .finish(),
            TxServerRequest::TxIdsNonBlocking { peer, ack, req, .. } => f
                .debug_struct("TxIdsNonBlocking")
                .field("peer", &peer.name)
                .field("ack", &ack)
                .field("req", &req)
                .finish(),
        }
    }
}

impl TxServerRequest {
    pub fn set_span(&mut self, span: Span) {
        match self {
            TxServerRequest::Txs { span: s, .. } => {
                *s = span;
            }
            TxServerRequest::TxIds { span: s, .. } => {
                *s = span;
            }
            TxServerRequest::TxIdsNonBlocking { span: s, .. } => {
                *s = span;
            }
        }
    }

    pub fn span(&self) -> &Span {
        match self {
            TxServerRequest::Txs { span, .. } => span,
            TxServerRequest::TxIds { span, .. } => span,
            TxServerRequest::TxIdsNonBlocking { span, .. } => span,
        }
    }

    pub fn peer(&self) -> &Peer {
        match self {
            TxServerRequest::Txs { peer, .. } => peer,
            TxServerRequest::TxIds { peer, .. } => peer,
            TxServerRequest::TxIdsNonBlocking { peer, .. } => peer,
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TxClientReply {
    Init {
        peer: Peer,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    TxIds {
        peer: Peer,
        tx_ids: Vec<(TxId, u32)>,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Txs {
        peer: Peer,
        txs: Vec<Tx>,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl TxClientReply {
    pub fn set_span(&mut self, span: Span) {
        match self {
            TxClientReply::Init { span: s, .. } => {
                *s = span;
            }
            TxClientReply::Txs { span: s, .. } => {
                *s = span;
            }
            TxClientReply::TxIds { span: s, .. } => {
                *s = span;
            }
        }
    }

    pub fn span(&self) -> &Span {
        match self {
            TxClientReply::Init { span, .. } => span,
            TxClientReply::Txs { span, .. } => span,
            TxClientReply::TxIds { span, .. } => span,
        }
    }

    pub fn peer(&self) -> &Peer {
        match self {
            TxClientReply::Init { peer, .. } => peer,
            TxClientReply::TxIds { peer, .. } => peer,
            TxClientReply::Txs { peer, .. } => peer,
        }
    }
}

impl fmt::Debug for TxClientReply {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TxClientReply::Init { peer, .. } => f.debug_struct("Init").field("peer", peer).finish(),
            TxClientReply::TxIds { peer, tx_ids, .. } => f
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
            TxClientReply::Txs { peer, txs, .. } => f
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
