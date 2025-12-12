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
use crate::consensus::tx_submission::TxSubmissionClientState;
use crate::consensus::tx_submission::tx_submission_client_state::TxClientResponse;
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::TxServerRequest;
use pure_stage::StageRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt::Debug;
use tracing::{Instrument, debug};

/// This stage processes incoming tx submission server requests
///
/// It keeps track of the state of each connected peer (advertised transactions, sequence numbers, etc...)
///
pub fn stage(
    (mut clients, errors): State,
    msg: TxServerRequest,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "tx_submission.receive_tx_request");
    async move {
        let peer = msg.peer().clone();
        let client = clients.get_client(&peer);
        let result = match client
            .process_tx_server_request(eff.mempool().as_ref(), msg)
            .await
        {
            Ok(TxClientResponse::Done) => {
                debug!(peer = %peer, "cannot serve any more transactions to this peer");
                clients.by_peer.remove(&peer);
                eff.network().send_tx_client_done(peer.clone()).await
            }
            Ok(TxClientResponse::ProtocolError(e)) => Err(e.into()),
            Ok(TxClientResponse::NextIds(tx_ids)) => {
                eff.network().send_tx_ids(peer.clone(), tx_ids).await
            }
            Ok(TxClientResponse::NextTxs(txs)) => eff.network().send_txs(peer.clone(), txs).await,
            Err(e) => Err(e.into()),
        };
        if let Err(e) = result {
            clients.by_peer.remove(&peer);
            debug!(peer = %peer, error = %e, "error processing tx server request");
        }
        (clients, errors)
    }
    .instrument(span)
}

type State = (Clients, StageRef<ProcessingFailed>);

/// List of client states, indexed by peer.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clients {
    by_peer: BTreeMap<Peer, TxSubmissionClientState>,
}

impl Debug for Clients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clients")
            .field("by_peer", &self.by_peer.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for Clients {
    fn default() -> Self {
        Clients::new()
    }
}

impl Clients {
    pub fn new() -> Self {
        Clients {
            by_peer: BTreeMap::new(),
        }
    }

    /// Return or initialize the state of the client for the given peer.
    pub fn get_client(&mut self, peer: &Peer) -> &mut TxSubmissionClientState {
        match self.by_peer.entry(peer.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(TxSubmissionClientState::new(peer)),
        }
    }
}
