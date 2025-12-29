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
    Inputs, Miniprotocol, Outcome, PROTO_N2N_TX_SUB, ProtocolState, Responder, StageState,
    miniprotocol, outcome,
};
use crate::tx_submission::{
    Blocking, Message, ResponderParams, TxSubmissionResponderState, TxSubmissionState,
};
use amaru_kernel::Tx;
use amaru_ouroboros_traits::{TxId, TxOrigin};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<TxSubmissionResponder>().boxed()]
}

pub fn responder() -> Miniprotocol<TxSubmissionState, TxSubmissionResponder, Responder> {
    miniprotocol(PROTO_N2N_TX_SUB.responder())
}

/// Result from protocol state when network message is received
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    Init,
    ReplyTxIds(Vec<(TxId, u32)>),
    ReplyTxs(Vec<Tx>),
    Done,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TxSubmissionResponder {
    state: TxSubmissionResponderState,
    muxer: StageRef<MuxMessage>,
}

impl TxSubmissionResponder {
    pub fn new(
        muxer: StageRef<MuxMessage>,
        params: ResponderParams,
        origin: TxOrigin,
    ) -> (TxSubmissionState, Self) {
        (
            TxSubmissionState::Init,
            Self {
                state: TxSubmissionResponderState::new(params, origin),
                muxer,
            },
        )
    }
}

impl AsRef<StageRef<MuxMessage>> for TxSubmissionResponder {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<TxSubmissionState, Responder> for TxSubmissionResponder {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &TxSubmissionState,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {}
    }

    async fn network(
        mut self,
        proto: &TxSubmissionState,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        let mempool = MemoryPool::new(eff.clone());
        
        // Convert ResponderResult back to Message for step() method
        let msg = match input {
            ResponderResult::Init => Message::Init,
            ResponderResult::ReplyTxIds(tx_ids) => Message::ReplyTxIds(tx_ids),
            ResponderResult::ReplyTxs(txs) => Message::ReplyTxs(txs),
            ResponderResult::Done => Message::Done,
        };
        
        // Use the existing step() method which handles all the business logic
        let (_new_proto_state, outcome) = self.state.step(&mempool, proto, msg).await?;
        
        // Convert the outcome to an action
        match outcome {
            crate::tx_submission::Outcome::Send(Message::RequestTxIds(ack, req, blocking)) => {
                Ok((Some(ResponderAction::SendRequestTxIds { ack, req, blocking }), self))
            }
            crate::tx_submission::Outcome::Send(Message::RequestTxs(tx_ids)) => {
                Ok((Some(ResponderAction::SendRequestTxs(tx_ids)), self))
            }
            crate::tx_submission::Outcome::Send(Message::Init | Message::ReplyTxIds(_) | Message::ReplyTxs(_) | Message::Done) => {
                // These messages should not be sent by responder
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

impl ProtocolState<Responder> for TxSubmissionState {
    type WireMsg = Message;
    type Action = ResponderAction;
    type Out = ResponderResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        // Responder waits for Init message, doesn't send anything on init
        Ok((outcome(), self.clone()))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok(match (self, input) {
            (TxSubmissionState::Init, Message::Init) => (
                outcome()
                    .want_next()
                    .result(ResponderResult::Init),
                TxSubmissionState::Init, // Stay in Init, stage state will handle transition
            ),
            (TxSubmissionState::TxIdsBlocking | TxSubmissionState::TxIdsNonBlocking, Message::ReplyTxIds(tx_ids)) => (
                outcome()
                    .want_next()
                    .result(ResponderResult::ReplyTxIds(tx_ids)),
                TxSubmissionState::Txs, // Transition to Txs state
            ),
            (TxSubmissionState::Txs, Message::ReplyTxs(txs)) => (
                outcome()
                    .want_next()
                    .result(ResponderResult::ReplyTxs(txs)),
                TxSubmissionState::Txs, // Stay in Txs, stage state will handle transition to TxIdsBlocking/NonBlocking
            ),
            (_, Message::Done) => (
                outcome().result(ResponderResult::Done),
                TxSubmissionState::Done,
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        Ok(match (self, input) {
            (TxSubmissionState::Init, ResponderAction::SendRequestTxIds { ack, req, blocking }) => {
                let new_state = if blocking == Blocking::Yes {
                    TxSubmissionState::TxIdsBlocking
                } else {
                    TxSubmissionState::TxIdsNonBlocking
                };
                (outcome().send(Message::RequestTxIds(ack, req, blocking)), new_state)
            }
            (TxSubmissionState::Txs, ResponderAction::SendRequestTxs(tx_ids)) => {
                (outcome().send(Message::RequestTxs(tx_ids)), TxSubmissionState::Txs)
            }
            (TxSubmissionState::TxIdsBlocking | TxSubmissionState::TxIdsNonBlocking, ResponderAction::SendRequestTxIds { ack, req, blocking }) => {
                let new_state = if blocking == Blocking::Yes {
                    TxSubmissionState::TxIdsBlocking
                } else {
                    TxSubmissionState::TxIdsNonBlocking
                };
                (outcome().send(Message::RequestTxIds(ack, req, blocking)), new_state)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum ResponderAction {
    SendRequestTxIds { ack: u16, req: u16, blocking: Blocking },
    SendRequestTxs(Vec<TxId>),
}

#[cfg(test)]
pub mod tests {
    // TODO: Implement protocol spec check similar to keepalive
    // This will require creating a spec() function first
}
