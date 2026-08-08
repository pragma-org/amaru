// Copyright 2026 PRAGMA
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

//! Peer-sharing initiator (client): non-pipelined request/response.

use std::net::SocketAddr;

use amaru_kernel::Peer;
use amaru_observability::debug_span;
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use tracing::Instrument;

use crate::{
    mux::MuxMessage,
    peer_sharing::{State, messages::Message},
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_PEER_SHARE, ProtocolState, StageState, miniprotocol,
        outcome,
    },
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<PeerSharingInitiator>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(State, PeerSharingInitiator)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerSharingMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ShareResult>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, PeerSharingInitiator, Initiator> {
    miniprotocol(PROTO_N2N_PEER_SHARE)
}

/// Local request to ask the remote peer for addresses.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSharingMessage {
    /// Request up to `amount` peers; reply is sent to `reply_to`.
    ///
    /// Non-pipelined: at most one request is in flight. A second request while busy is queued
    /// (replacing any previous queued request) and sent only after the current reply arrives.
    ShareRequest { amount: u8, reply_to: StageRef<ShareResult> },
}

/// Reply delivered to the requester after `MsgSharePeers`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShareResult {
    pub peers: Vec<SocketAddr>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSharingInitiator {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    /// In-flight request: (requested amount, reply destination).
    in_flight: Option<(u8, StageRef<ShareResult>)>,
    /// At most one queued request while busy (non-pipelined).
    pending: Option<(u8, StageRef<ShareResult>)>,
}

impl PeerSharingInitiator {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, conn_id: ConnectionId) -> (State, Self) {
        (State::Idle, Self { muxer, peer, conn_id, in_flight: None, pending: None })
    }
}

impl StageState<State, Initiator> for PeerSharingInitiator {
    type LocalIn = PeerSharingMessage;

    async fn local(
        mut self,
        proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        match input {
            PeerSharingMessage::ShareRequest { amount, reply_to } => match proto {
                State::Idle => {
                    debug_assert!(self.in_flight.is_none());
                    self.in_flight = Some((amount, reply_to));
                    Ok((Some(InitiatorAction::ShareRequest { amount }), self))
                }
                State::Busy => {
                    // Non-pipelined: do not send while waiting; keep only the latest pending.
                    if self.pending.is_some() {
                        tracing::debug!(
                            peer = %self.peer,
                            conn_id = %self.conn_id,
                            "replacing queued peer-sharing request"
                        );
                    }
                    self.pending = Some((amount, reply_to));
                    Ok((None, self))
                }
                State::Done => anyhow::bail!("peer-sharing initiator already done"),
            },
        }
    }

    async fn network(
        mut self,
        _proto: &State,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        let span = debug_span!(
            protocols::peer_sharing::initiator::PEER_SHARING_INITIATOR_STAGE,
            peer = self.peer.to_string(),
            conn_id = self.conn_id.to_string(),
        );
        async move {
            match input {
                InitiatorResult::SharePeers { peers } => {
                    let Some((amount, reply_to)) = self.in_flight.take() else {
                        tracing::warn!("received SharePeers without in-flight request; terminating");
                        return eff.terminate().await;
                    };
                    if peers.len() > amount as usize {
                        tracing::warn!(
                            requested = amount,
                            received = peers.len(),
                            "peer returned more addresses than requested; terminating"
                        );
                        return eff.terminate().await;
                    }
                    eff.send(&reply_to, ShareResult { peers }).await;

                    if let Some((amount, reply_to)) = self.pending.take() {
                        self.in_flight = Some((amount, reply_to));
                        Ok((Some(InitiatorAction::ShareRequest { amount }), self))
                    } else {
                        Ok((None, self))
                    }
                }
            }
        }
        .instrument(span)
        .await
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Initiator> for State {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        // Client agency in Idle: wait for a local ShareRequest (no WantNext until we send).
        Ok((outcome(), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = debug_span!(
            protocols::peer_sharing::initiator::PEER_SHARING_INITIATOR_PROTOCOL,
            message_type = input.message_type()
        )
        .entered();
        use State::*;

        Ok(match (self, input) {
            (Busy, Message::SharePeers { peers }) => (outcome().result(InitiatorResult::SharePeers { peers }), Idle),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Idle, InitiatorAction::ShareRequest { amount }) => {
                (outcome().send(Message::ShareRequest { amount }).want_next(), Busy)
            }
            (Idle, InitiatorAction::Done) => (outcome().send(Message::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum InitiatorAction {
    ShareRequest { amount: u8 },
    Done,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    SharePeers { peers: Vec<SocketAddr> },
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Initiator;

    #[test]
    fn test_initiator_protocol() {
        crate::peer_sharing::spec::<Initiator>().check(State::Idle, |msg| match msg {
            Message::ShareRequest { amount } => Some(InitiatorAction::ShareRequest { amount: *amount }),
            Message::Done => Some(InitiatorAction::Done),
            Message::SharePeers { .. } => None,
        });
    }
}
