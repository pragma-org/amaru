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

use crate::mempool_effects::MemoryPool;
use crate::mux::MuxMessage;
use crate::protocol::{
    Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_TX_SUB, ProtocolState, StageState,
    miniprotocol, outcome,
};
use crate::tx_submission::{Blocking, Message, TxSubmissionInitiatorState, TxSubmissionState};
use amaru_kernel::Tx;
use amaru_ouroboros_traits::TxId;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<InitiatorMessage>().boxed(),
        pure_stage::register_data_deserializer::<TxSubmissionInitiator>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<TxSubmissionState, TxSubmissionInitiator, Initiator> {
    miniprotocol(PROTO_N2N_TX_SUB)
}

/// Message sent to the handler (currently unused, but kept for future extensibility)
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorMessage {
    // Future: could add messages to trigger actions
}

/// Result from protocol state when network message is received
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    RequestTxIds {
        ack: u16,
        req: u16,
        blocking: Blocking,
    },
    RequestTxs(Vec<TxId>),
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TxSubmissionInitiator {
    state: TxSubmissionInitiatorState,
    muxer: StageRef<MuxMessage>,
}

impl TxSubmissionInitiator {
    pub fn new(muxer: StageRef<MuxMessage>) -> (TxSubmissionState, Self) {
        (
            TxSubmissionState::Init,
            Self {
                state: TxSubmissionInitiatorState::new(),
                muxer,
            },
        )
    }
}

impl AsRef<StageRef<MuxMessage>> for TxSubmissionInitiator {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<TxSubmissionState, Initiator> for TxSubmissionInitiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        self,
        _proto: &TxSubmissionState,
        _input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        // Currently no local inputs are handled
        Ok((None, self))
    }

    async fn network(
        mut self,
        proto: &TxSubmissionState,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        let mempool = MemoryPool::new(eff.clone());

        // Convert InitiatorResult back to Message for step() method
        let msg = match input {
            InitiatorResult::RequestTxIds { ack, req, blocking } => {
                Message::RequestTxIds(ack, req, blocking)
            }
            InitiatorResult::RequestTxs(tx_ids) => Message::RequestTxs(tx_ids),
        };

        // Use the existing step() method which handles all the business logic
        let (_new_proto_state, outcome) = self.state.step(&mempool, proto, msg).await?;

        // Convert the outcome to an action
        match outcome {
            crate::tx_submission::Outcome::Send(Message::ReplyTxIds(tx_ids)) => {
                Ok((Some(InitiatorAction::SendReplyTxIds(tx_ids)), self))
            }
            crate::tx_submission::Outcome::Send(Message::ReplyTxs(txs)) => {
                Ok((Some(InitiatorAction::SendReplyTxs(txs)), self))
            }
            crate::tx_submission::Outcome::Send(Message::Init) => {
                // Init message is sent by protocol state on init, not by stage state
                Ok((None, self))
            }
            crate::tx_submission::Outcome::Send(
                Message::RequestTxIds(_, _, _) | Message::RequestTxs(_) | Message::Done,
            ) => {
                // These messages should not be sent by initiator
                anyhow::bail!("unexpected message in outcome: {:?}", outcome);
            }
            crate::tx_submission::Outcome::Done => {
                // Protocol is done, no action needed
                Ok((None, self))
            }
            crate::tx_submission::Outcome::Error(err) => {
                anyhow::bail!("protocol error: {}", err);
            }
        }
    }
}

impl ProtocolState<Initiator> for TxSubmissionState {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        // On init, send Init message and transition to Idle
        Ok((outcome().send(Message::Init), TxSubmissionState::Idle))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok(match (self, input) {
            (TxSubmissionState::Idle, Message::RequestTxIds(ack, req, blocking)) => (
                outcome()
                    .want_next()
                    .result(InitiatorResult::RequestTxIds { ack, req, blocking }),
                TxSubmissionState::Idle, // Stay in Idle, stage state will handle the response
            ),
            (TxSubmissionState::Idle, Message::RequestTxs(tx_ids)) => (
                outcome()
                    .want_next()
                    .result(InitiatorResult::RequestTxs(tx_ids)),
                TxSubmissionState::Idle, // Stay in Idle, stage state will handle the response
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        Ok(match (self, input) {
            (TxSubmissionState::Idle, InitiatorAction::SendReplyTxIds(tx_ids)) => (
                outcome().send(Message::ReplyTxIds(tx_ids)),
                TxSubmissionState::Idle,
            ),
            (TxSubmissionState::Idle, InitiatorAction::SendReplyTxs(txs)) => (
                outcome().send(Message::ReplyTxs(txs)),
                TxSubmissionState::Idle,
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum InitiatorAction {
    SendReplyTxIds(Vec<(TxId, u32)>),
    SendReplyTxs(Vec<Tx>),
}

#[cfg(test)]
pub mod tests {
    // TODO: Implement protocol spec check similar to keepalive
    // This will require creating a spec() function first
}
