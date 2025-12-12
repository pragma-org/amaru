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
use amaru_kernel::{Hash, Tx, to_cbor};
use amaru_ouroboros_traits::{TxClientReply, TxId, TxServerRequest};
use minicbor::decode;
use pallas_network::miniprotocols::txsubmission::{
    EraTxBody, EraTxId, Reply, Request, TxIdAndSize,
};
use pallas_traverse::Era;
use tracing::Span;

/// Retrieves the TxId from an EraTxId.
pub fn tx_id_from_era_tx_id(era_tx_id: &EraTxId) -> TxId {
    TxId::new(Hash::from(era_tx_id.1.as_slice()))
}

/// Retrieves a Tx from an EraTxBody.
pub fn tx_from_era_tx_body(era_tx: &EraTxBody) -> Result<Tx, decode::Error> {
    minicbor::decode(era_tx.1.as_slice())
}

/// Extract a TxId and size from a TxIdAndSize.
pub fn tx_id_and_size(ts: TxIdAndSize<EraTxId>) -> (TxId, u32) {
    (TxId::new(Hash::from(ts.0.1.as_slice())), ts.1)
}

/// Create a new EraTxId for the Conway era.
pub fn era_tx_id(tx_id: TxId) -> EraTxId {
    EraTxId(Era::Conway.into(), tx_id.to_vec())
}

/// Create a new EraTxBody for the Conway era.
pub fn era_tx_body(tx: &Tx) -> EraTxBody {
    era_tx_body_from_vec(to_cbor(tx))
}

/// Create a new EraTxBody for the Conway era.
pub fn era_tx_body_from_vec(tx_body: Vec<u8>) -> EraTxBody {
    EraTxBody(Era::Conway.into(), tx_body)
}

pub fn era_tx_id_to_string(era_tx_id: &EraTxId) -> String {
    Hash::<32>::from(era_tx_id.1.as_slice()).to_string()
}

pub fn era_tx_body_to_string(era_tx_body: &EraTxBody) -> String {
    String::from_utf8_lossy(&era_tx_body.1).to_string()
}

pub fn era_tx_ids(tx_ids: &[TxId]) -> Vec<EraTxId> {
    let mut era_tx_ids: Vec<EraTxId> = Vec::new();
    for tx_id in tx_ids {
        era_tx_ids.push(era_tx_id(*tx_id));
    }
    era_tx_ids
}

pub fn era_tx_bodies(txs: &[Tx]) -> Vec<EraTxBody> {
    let mut era_txs_bodies: Vec<EraTxBody> = Vec::new();
    for tx in txs {
        era_txs_bodies.push(era_tx_body(tx));
    }
    era_txs_bodies
}

pub fn from_pallas_reply(
    peer: &Peer,
    reply: Reply<EraTxId, EraTxBody>,
) -> anyhow::Result<TxClientReply> {
    match reply {
        Reply::TxIds(tx_ids) => {
            let ids_and_sizes = tx_ids
                .into_iter()
                .map(|ts| {
                    let (tx_id, size) = tx_id_and_size(ts);
                    (tx_id, size)
                })
                .collect();
            Ok(TxClientReply::TxIds {
                peer: peer.clone(),
                tx_ids: ids_and_sizes,
                span: Span::current(),
            })
        }
        Reply::Txs(txs) => {
            let txs_converted = txs
                .into_iter()
                .map(|era_tx| tx_from_era_tx_body(&era_tx))
                .collect::<Result<Vec<Tx>, _>>()?;
            Ok(TxClientReply::Txs {
                peer: peer.clone(),
                txs: txs_converted,
                span: Span::current(),
            })
        }
        Reply::Done => Ok(TxClientReply::Done {
            peer: peer.clone(),
            span: Span::current(),
        }),
    }
}

pub fn to_pallas_request(peer: &Peer, request: Request<EraTxId>) -> TxServerRequest {
    match request {
        Request::TxIds(ack, req) => TxServerRequest::TxIds {
            peer: peer.clone(),
            ack,
            req,
            span: Span::current(),
        },
        Request::TxIdsNonBlocking(ack, req) => TxServerRequest::TxIdsNonBlocking {
            peer: peer.clone(),
            ack,
            req,
            span: Span::current(),
        },
        Request::Txs(tx_ids) => TxServerRequest::Txs {
            peer: peer.clone(),
            tx_ids: tx_ids
                .into_iter()
                .map(|era_tx_id| tx_id_from_era_tx_id(&era_tx_id))
                .collect(),
            span: Span::current(),
        },
    }
}
