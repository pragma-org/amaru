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

use crate::consensus::effects::BaseOps;
use crate::consensus::effects::ConsensusOps;
use crate::consensus::effects::NetworkOps;
use crate::consensus::errors::ProcessingFailed;
use amaru_kernel::connection::ClientConnectionError;
use amaru_kernel::peer::Peer;
use amaru_kernel::to_cbor;
use amaru_kernel::tx_submission_events::TxClientReply;
use amaru_network::tx_submission::tx_submission_server::TxResponse;
use amaru_network::tx_submission::{
    ServerParams, TxSubmissionServerState, new_era_tx_body, new_era_tx_id, tx_id_from_era_tx_id,
};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Reply, TxIdAndSize};
use pure_stage::StageRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::Instrument;

type State = (Servers, StageRef<ProcessingFailed>);

pub fn stage(
    (mut servers, errors): State,
    msg: TxClientReply,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "tx_submission.receive_tx_reply");
    async move {
        let peer = msg.peer();
        match msg {
            TxClientReply::Init { .. } => {
                if let Err(e) = servers.init_peer(peer, &eff).await {
                    let processing_failed = ProcessingFailed::new(peer, e.to_anyhow());
                    let _ = eff.base().send(&errors, processing_failed).await;
                }
            }
            TxClientReply::TxIds { .. } | TxClientReply::Txs { .. } => {
                if let Some(server) = servers.by_peer.get_mut(peer) {
                    let result = match server
                        .process_tx_reply(eff.mempool(), to_network_reply(&msg))
                        .await
                    {
                        Ok(TxResponse::Done) => {
                            servers.by_peer.remove(peer);
                            Ok(())
                        }
                        Ok(TxResponse::NextTxIds(ack, req)) => {
                            eff.network().request_tx_ids(peer.clone(), ack, req).await
                        }
                        Ok(TxResponse::NextTxs(Some(tx_ids))) => {
                            eff.network()
                                .request_txs(
                                    peer.clone(),
                                    tx_ids.iter().map(tx_id_from_era_tx_id).collect(),
                                )
                                .await
                        }
                        Ok(TxResponse::NextTxs(None)) => Ok(()),
                        Err(e) => Err(e.into()),
                    };
                    if let Err(e) = result {
                        let processing_failed = ProcessingFailed::new(peer, e.to_anyhow());
                        let _ = eff.base().send(&errors, processing_failed).await;
                    }
                }
            }
        }
        (servers, errors)
    }
    .instrument(span)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Servers {
    server_params: ServerParams,
    by_peer: BTreeMap<Peer, TxSubmissionServerState>,
}

impl Debug for Servers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Servers")
            .field("server_params", &self.server_params)
            .field("by_peer", &self.by_peer.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Servers {
    pub fn new(server_params: ServerParams) -> Self {
        Servers {
            server_params,
            by_peer: BTreeMap::new(),
        }
    }

    pub async fn init_peer(
        &mut self,
        peer: &Peer,
        eff: &impl ConsensusOps,
    ) -> Result<(), ClientConnectionError> {
        let mut state = TxSubmissionServerState::new(peer, self.server_params.clone());
        let (ack, req) = state.request_tx_ids(eff.mempool()).await?;
        eff.network().request_tx_ids(peer.clone(), ack, req).await?;
        let _ = self.by_peer.insert(peer.clone(), state);
        Ok(())
    }
}

fn to_network_reply(tx_reply: &TxClientReply) -> Reply<EraTxId, EraTxBody> {
    match tx_reply {
        TxClientReply::Txs { txs, .. } => {
            Reply::Txs(txs.iter().map(|tx| new_era_tx_body(to_cbor(tx))).collect())
        }
        TxClientReply::TxIds { tx_ids, .. } => Reply::TxIds(
            tx_ids
                .iter()
                .map(|(tx_id, size)| TxIdAndSize(new_era_tx_id(tx_id.clone()), *size))
                .collect(),
        ),
        TxClientReply::Init { .. } => unreachable!(),
    }
}
