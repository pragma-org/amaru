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
use crate::consensus::tx_submission::TxSubmissionClientState;
use crate::consensus::tx_submission::tx_submission_client_state::TxClientResponse;
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::TxServerRequest;
use pure_stage::StageRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt::Debug;
use tracing::Instrument;

type State = (Clients, StageRef<ProcessingFailed>);

pub fn stage(
    (mut clients, errors): State,
    msg: TxServerRequest,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "tx_submission.receive_tx_request");
    async move {
        let peer = msg.peer().clone();
        let client = clients.get_client_mut(&peer);
        let result = match client.process_tx_request(eff.mempool(), msg).await {
            Ok(TxClientResponse::Done) => {
                clients.by_peer.remove(&peer);
                Ok(())
            }
            Ok(TxClientResponse::NextIds(tx_ids)) => {
                eff.network().send_tx_ids(peer.clone(), tx_ids).await
            }
            Ok(TxClientResponse::NextTxs(txs)) => {
                eff.network()
                    .send_txs(
                        peer.clone(),
                        txs.into_iter().map(|tx| tx.as_ref().clone()).collect(),
                    )
                    .await
            }
            Err(e) => Err(e.into()),
        };
        match result {
            Ok(_) => {}
            Err(e) => {
                let processing_failed = ProcessingFailed::new(&peer, e.to_anyhow());
                let _ = eff.base().send(&errors, processing_failed).await;
            }
        }
        (clients, errors)
    }
    .instrument(span)
}

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

    pub fn get_client_mut(&mut self, peer: &Peer) -> &mut TxSubmissionClientState {
        match self.by_peer.entry(peer.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(TxSubmissionClientState::new(peer)),
        }
    }
}
