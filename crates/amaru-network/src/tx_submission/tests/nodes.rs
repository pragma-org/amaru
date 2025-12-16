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

use crate::tx_submission::Message::Init;
use crate::tx_submission::tests::NodesOptions;
use crate::tx_submission::{
    Message, Outcome, TxSubmissionInitiatorState, TxSubmissionResponderState, TxSubmissionState,
};
use amaru_ouroboros_traits::Mempool;
use pallas_primitives::conway::Tx;
use std::sync::Arc;

/// A set of two nodes where one acts as an initiator and the other as a responder.
/// They have their own mempools and states.
pub struct Nodes {
    agency: Agency,
    message: Message,
    initiator_protocol_state: TxSubmissionState,
    responder_protocol_state: TxSubmissionState,
    initiator_state: TxSubmissionInitiatorState,
    responder_state: TxSubmissionResponderState,
    pub initiator_mempool: Arc<dyn Mempool<Tx>>,
    pub responder_mempool: Arc<dyn Mempool<Tx>>,
    pub outcomes: Vec<Outcome>,
}

impl Nodes {
    /// Creates two nodes with the specified options.
    pub fn new(nodes_options: NodesOptions) -> Nodes {
        let initiator_mempool = nodes_options.initiator_mempool;
        let initiator_state = TxSubmissionInitiatorState::new();
        let responder_state = TxSubmissionResponderState::new(nodes_options.responder_params);
        let responder_mempool = nodes_options.responder_mempool;
        Nodes {
            agency: Agency::Initiator,
            message: Init,
            initiator_protocol_state: TxSubmissionState::Init,
            responder_protocol_state: TxSubmissionState::Init,
            initiator_state,
            responder_state,
            initiator_mempool,
            responder_mempool,
            outcomes: vec![],
        }
    }

    /// Inserts transactions into the initiator's mempool in order to serve them to the responder.
    pub fn insert_client_transactions(&self, txs: &[Tx]) {
        for tx in txs.iter() {
            self.initiator_mempool
                .insert(tx.clone(), amaru_ouroboros_traits::TxOrigin::Remote)
                .unwrap();
        }
    }

    /// Starts the interaction between the initiator and the responder until completion.
    pub async fn start(&mut self) -> anyhow::Result<()> {
        let mut outcome = Outcome::Send(Init);
        while matches!(outcome, Outcome::Send(_)) {
            outcome = self.step().await?;
        }
        Ok(())
    }

    /// Send a single message between the initiator and the responder, updating their states accordingly.
    pub async fn step(&mut self) -> anyhow::Result<Outcome> {
        let outcome = match self.agency {
            Agency::Initiator => {
                let (new_protocol_state, outcome) = self
                    .initiator_state
                    .step(
                        self.initiator_mempool.as_ref(),
                        &self.initiator_protocol_state,
                        self.message.clone(),
                    )
                    .await?;
                self.initiator_protocol_state = new_protocol_state;
                self.agency = Agency::Responder;
                outcome
            }
            Agency::Responder => {
                let (new_protocol_state, outcome) = self
                    .responder_state
                    .step(
                        self.responder_mempool.as_ref(),
                        &self.responder_protocol_state,
                        self.message.clone(),
                    )
                    .await?;
                self.responder_protocol_state = new_protocol_state;
                self.agency = Agency::Initiator;
                outcome
            }
        };

        match &outcome {
            Outcome::Done => {
                self.outcomes.push(Outcome::Done);
            }
            Outcome::Error(e) => {
                self.outcomes.push(Outcome::Error(e.clone()));
            }
            Outcome::Send(message) => {
                self.message = message.clone();
                self.outcomes.push(Outcome::Send(message.clone()))
            }
        }
        Ok(outcome)
    }
}

/// This tracks which node has the agency to send the next message.
enum Agency {
    Initiator,
    Responder,
}
