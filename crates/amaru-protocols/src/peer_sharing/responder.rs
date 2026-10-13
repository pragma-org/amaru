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
//! On `MsgShareRequest`, asks the peer-tracking resource for addresses and replies with
//! `MsgSharePeers`. The reply is a class C9 read, so this stage awaits the resource.

use std::net::SocketAddr;

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug_span, info};
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void};

use crate::{
    mux::MuxMessage,
    peer_sharing::{State, messages::Message},
    peer_tracking_effects::PeerTrack,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_PEER_SHARE, ProtocolState, Responder, StageState, miniprotocol,
        outcome,
    },
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<PeerSharingResponder>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(State, PeerSharingResponder)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ResponderResult>().boxed(),
    ]
}

pub fn responder() -> Miniprotocol<State, PeerSharingResponder, Responder> {
    miniprotocol(PROTO_N2N_PEER_SHARE.responder())
}

/// Register the peer-sharing **responder** (server) on the mux.
///
/// On `MsgShareRequest`, the responder reads addresses from the peer-tracking resource and sends
/// `MsgSharePeers`.
pub async fn register_peer_sharing_responder<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<Void> {
    use crate::{mux::Frame, protocol::ingress_limit};

    let (state, stage) = PeerSharingResponder::new(muxer.clone(), peer);
    let ps = eff.stage("peer_sharing-responder", responder()).await;
    let ps = eff.supervise(ps, tombstone);
    let ps = eff.wire_up(ps, (state, stage)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_PEER_SHARE.responder().erase(),
            frame: Frame::OneCborItem,
            handler: ps.contramap(Inputs::<Void>::Network),
            max_buffer: ingress_limit(PROTO_N2N_PEER_SHARE.responder()),
        },
    )
    .await;
    ps.contramap(Inputs::<Void>::Local)
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSharingResponder {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
}

impl PeerSharingResponder {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer) -> (State, Self) {
        (State::Idle, Self { muxer, peer })
    }
}

impl StageState<State, Responder> for PeerSharingResponder {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {}
    }

    async fn network(
        self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::ShareRequest { amount } => {
                let span = debug_span!(protocols::peer_sharing::responder::PEER_SHARING_RESPONDER_STAGE, amount);
                async move {
                    let peer = self.peer;
                    let now = eff.clock().await;
                    let track = PeerTrack::new(eff);
                    let peers = track.query_share_peers(peer, amount, now).await;
                    if peers.len() > usize::from(amount) {
                        anyhow::bail!("cannot share {} peers when only {amount} were requested", peers.len());
                    }
                    track.record_share_request_served(peer, amount, now).await;
                    let peers_list = peers.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                    let count = peers.len();
                    info!(
                        protocols::peer_selection::sharing::SENT,
                        peer,
                        peers = peers_list,
                        requested = amount,
                        count,
                    );
                    Ok((Some(ResponderAction::SharePeers { peers }), self))
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
            (Idle, Message::Done) => (outcome().want_next(), Idle),
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
