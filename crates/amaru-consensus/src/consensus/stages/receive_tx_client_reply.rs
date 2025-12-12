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

use crate::consensus::effects::ConsensusOps;
use crate::consensus::effects::NetworkOps;
use crate::consensus::errors::ProcessingFailed;
use crate::consensus::tx_submission::{Blocking, ServerParams, TxSubmissionServerState};
use amaru_kernel::connection::ClientConnectionError;
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::TxClientReply;
use pure_stage::StageRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::{Instrument, debug};

/// This stage processes incoming tx submission client replies:
///
///  - It keeps track of connected peers.
///  - For each peer, it maintains some state about the requests that have made to the client.
///
pub fn stage(
    (mut servers, errors): State,
    msg: TxClientReply,
    eff: impl ConsensusOps,
) -> impl Future<Output=State> {
    let span = tracing::trace_span!(parent: msg.span(), "tx_submission.receive_tx_client_reply");
    async move {
        let peer = msg.peer().clone();
        match msg {
            TxClientReply::Init { .. } => {
                if let Err(e) = servers.init_peer(&peer, &eff).await {
                    debug!(peer = %peer, error = %e, "error processing tx client init");
                }
            }
            TxClientReply::Done { .. } => {
                servers.remove_peer(&peer);
            }
            TxClientReply::TxIds { tx_ids, .. } => {
                if let Some(server) = servers.by_peer.get_mut(&peer) {
                    let result = match server.process_tx_ids_reply(eff.mempool().as_ref(), tx_ids) {
                        Ok(None) => {
                            // Client indicated it is done; remove the peer.
                            servers.remove_peer(&peer);
                            Ok(())
                        }
                        Ok(Some(requested_txs)) => {
                            eff.network().request_txs(peer.clone(), requested_txs).await
                        }
                        Err(e) => Err(e.into()),
                    };
                    if let Err(e) = result {
                        debug!(peer = %peer, error = %e, message = %e, "error processing tx client reply");
                    }
                } else {
                    debug!(peer = %peer, "a client reply with tx ids or txs was received from an uninitialized peer");
                }
            }
            TxClientReply::Txs { txs, .. } => {
                if let Some(server) = servers.by_peer.get_mut(&peer) {
                    let result = match server.process_txs_reply(eff.mempool().as_ref(), txs) {
                        Ok((ack, req, blocking)) => {
                            eff.network().request_tx_ids(peer.clone(), ack, req, blocking).await
                        }
                        Err(e) => Err(e.into()),
                    };
                    if let Err(e) = result {
                        servers.remove_peer(&peer);
                        debug!(peer = %peer, error = %e, message = %e, "error processing tx client reply");
                    }
                } else {
                    debug!(peer = %peer, "a client reply with tx ids or txs was received from an uninitialized peer");
                }
            }
        }
        (servers, errors)
    }
        .instrument(span)
}

type State = (Servers, StageRef<ProcessingFailed>);

/// List of server state for each connected peer.
/// The parameters are used to control batch sizes, window sizes, timeouts, etc.
///
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

    /// Create the initial state for a newly connected peer, and send the first request for tx ids.
    pub async fn init_peer(
        &mut self,
        peer: &Peer,
        eff: &impl ConsensusOps,
    ) -> Result<(), ClientConnectionError> {
        let mut state = TxSubmissionServerState::new(peer, self.server_params.clone());
        // The first request is always blocking.
        let (ack, req, blocking) = state.request_tx_ids(eff.mempool().as_ref());
        eff.network()
            .request_tx_ids(peer.clone(), ack, req, blocking)
            .await?;
        let _ = self.by_peer.insert(peer.clone(), state);
        Ok(())
    }

    /// Remove the state for a disconnected peer.
    pub fn remove_peer(&mut self, peer: &Peer) {
        self.by_peer.remove(peer);
    }
}
