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

//! Peer-sharing responder (server).
//!
//! On `MsgShareRequest`, forwards the query via the manager to peer selection and replies with
//! `MsgSharePeers` once a [`SharePeersReply`] arrives.

use std::net::SocketAddr;

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug_span};
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void};

use crate::{
    manager::ManagerMessage,
    mux::MuxMessage,
    peer_sharing::{SharePeersReply, State, messages::Message},
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_PEER_SHARE, ProtocolState, Responder, StageState, miniprotocol,
        outcome,
    },
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<PeerSharingResponder>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(State, PeerSharingResponder)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ResponderMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ResponderResult>().boxed(),
        amaru_pure_stage::register_data_deserializer::<SharePeersReply>().boxed(),
    ]
}

pub fn responder() -> Miniprotocol<State, PeerSharingResponder, Responder> {
    miniprotocol(PROTO_N2N_PEER_SHARE.responder())
}

/// Register the peer-sharing **responder** (server) on the mux.
///
/// On `MsgShareRequest`, the responder asks the manager to query peer selection and waits for
/// a [`SharePeersReply`] before sending `MsgSharePeers`.
pub async fn register_peer_sharing_responder<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    manager: StageRef<ManagerMessage>,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<ResponderMessage> {
    use crate::{mux::Frame, peer_sharing::MAX_MESSAGE_BYTES};

    let (state, stage) = PeerSharingResponder::new(muxer.clone(), peer, manager);
    let ps = eff.stage("peer_sharing", responder()).await;
    let ps = eff.supervise(ps, tombstone);
    let ps = eff.wire_up(ps, (state, stage)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_PEER_SHARE.responder().erase(),
            frame: Frame::OneCborItem,
            handler: ps.contramap(Inputs::<ResponderMessage>::Network),
            max_buffer: MAX_MESSAGE_BYTES,
        },
    )
    .await;
    ps.contramap(Inputs::<ResponderMessage>::Local)
}

/// Local messages into the responder stage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResponderMessage {
    /// Supply peers for the outstanding share request (must not exceed the requested amount).
    SharePeers { peers: Vec<SocketAddr> },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSharingResponder {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    manager: StageRef<ManagerMessage>,
    /// Bridges [`SharePeersReply`] from peer selection into [`ResponderMessage::SharePeers`].
    /// Created lazily on the first request.
    reply_bridge: Option<StageRef<SharePeersReply>>,
    /// Requested amount while waiting for a local [`ResponderMessage::SharePeers`].
    awaiting: Option<u8>,
}

impl PeerSharingResponder {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, manager: StageRef<ManagerMessage>) -> (State, Self) {
        (State::Idle, Self { muxer, peer, manager, reply_bridge: None, awaiting: None })
    }
}

impl StageState<State, Responder> for PeerSharingResponder {
    type LocalIn = ResponderMessage;

    async fn local(
        mut self,
        _proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderMessage::SharePeers { peers } => {
                let Some(amount) = self.awaiting.take() else {
                    anyhow::bail!("SharePeers without a pending request");
                };
                if peers.len() > amount as usize {
                    anyhow::bail!("cannot share {} peers when only {} were requested", peers.len(), amount);
                }
                Ok((Some(ResponderAction::SharePeers { peers }), self))
            }
        }
    }

    async fn network(
        mut self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::ShareRequest { amount } => {
                let span = debug_span!(protocols::peer_sharing::responder::PEER_SHARING_RESPONDER_STAGE, amount);
                async move {
                    let reply_to = match self.reply_bridge.clone() {
                        Some(bridge) => bridge,
                        None => {
                            let bridge = eff.me_ref().contramap(|r: SharePeersReply| {
                                Inputs::Local(ResponderMessage::SharePeers { peers: r.peers })
                            });
                            self.reply_bridge = Some(bridge.clone());
                            bridge
                        }
                    };
                    self.awaiting = Some(amount);
                    eff.send(&self.manager, ManagerMessage::ShareRequest { peer: self.peer.clone(), amount, reply_to })
                        .await;
                    Ok((None, self))
                }
                .instrument(span)
                .await
            }
            ResponderResult::Done => Ok((None, self)),
        }
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Responder> for State {
    type WireMsg = Message;
    type Action = ResponderAction;
    type Out = ResponderResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        // Server waits for MsgShareRequest or MsgDone.
        Ok((outcome().want_next(), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = debug_span!(
            protocols::peer_sharing::responder::PEER_SHARING_RESPONDER_PROTOCOL,
            message_type = input.message_type()
        );
        let _guard = _span.enter();
        use State::*;

        Ok(match (self, input) {
            (Idle, Message::ShareRequest { amount }) => {
                (outcome().result(ResponderResult::ShareRequest { amount }), Busy)
            }
            (Idle, Message::Done) => (outcome().result(ResponderResult::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Busy, ResponderAction::SharePeers { peers }) => {
                (outcome().send(Message::SharePeers { peers }).want_next(), Idle)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum ResponderAction {
    SharePeers { peers: Vec<SocketAddr> },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    ShareRequest { amount: u8 },
    Done,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Responder;

    #[test]
    fn test_responder_protocol() {
        crate::peer_sharing::spec::<Responder>().check(State::Idle, |msg| match msg {
            Message::SharePeers { peers } => Some(ResponderAction::SharePeers { peers: peers.clone() }),
            Message::ShareRequest { .. } | Message::Done => None,
        });
    }
}
