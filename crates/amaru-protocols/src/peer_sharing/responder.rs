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
use amaru_ouroboros::{ConnectionId, RemoteProtocol};
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void};

use crate::{
    mux::MuxMessage,
    peer_sharing::{SHARE_POLICY_MAX, State, messages::Message},
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
    conn_id: ConnectionId,
    own_address: Option<SocketAddr>,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<Void> {
    use crate::{mux::Frame, protocol::ingress_limit};

    let (state, stage) = PeerSharingResponder::new(muxer.clone(), peer, conn_id, own_address);
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
    conn_id: ConnectionId,
    /// This bearer's local IP with the listen port. Absent when the node is not listening
    /// or the local IP is unspecified.
    own_address: Option<SocketAddr>,
}

impl PeerSharingResponder {
    pub fn new(
        muxer: StageRef<MuxMessage>,
        peer: Peer,
        conn_id: ConnectionId,
        own_address: Option<SocketAddr>,
    ) -> (State, Self) {
        (State::Idle, Self { muxer, peer, conn_id, own_address })
    }
}

/// Append `own` to a share sample without exceeding `amount` or [`SHARE_POLICY_MAX`].
///
/// A full sample drops its last address to make room. An unspecified address, the
/// requester, or a missing address is left out. An address already in the sample is
/// not repeated.
fn include_own_address(
    mut peers: Vec<SocketAddr>,
    own: Option<SocketAddr>,
    amount: u8,
    requester: SocketAddr,
) -> Vec<SocketAddr> {
    let cap = usize::from(amount.min(SHARE_POLICY_MAX));
    peers.truncate(cap);
    let Some(own) = own.filter(|addr| !addr.ip().is_unspecified() && *addr != requester) else {
        return peers;
    };
    if cap == 0 || peers.contains(&own) {
        return peers;
    }
    if peers.len() >= cap {
        peers.pop();
    }
    peers.push(own);
    peers
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
                    PeerTrack::new(eff)
                        .note_remote_protocol(self.peer, self.conn_id, RemoteProtocol::PeerSharing, true)
                        .await;
                    let peer = self.peer;
                    let now = eff.clock().await;
                    let track = PeerTrack::new(eff);
                    let peers = include_own_address(
                        track.query_share_peers(peer, amount, now).await,
                        self.own_address,
                        amount,
                        SocketAddr::from(peer),
                    );
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
                .instrument(span.into())
                .await
            }
            ResponderResult::Done => {
                PeerTrack::new(eff)
                    .note_remote_protocol(self.peer, self.conn_id, RemoteProtocol::PeerSharing, false)
                    .await;
                Ok((None, self))
            }
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
            (Idle, Message::Done) => (outcome().result(ResponderResult::Done).want_next(), Idle),
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

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn own_address_is_appended_inside_the_requested_amount() {
        let own = addr(3000);
        let peers = include_own_address(vec![addr(1), addr(2)], Some(own), 4, addr(9));
        assert_eq!(peers, vec![addr(1), addr(2), own]);
    }

    #[test]
    fn own_address_replaces_the_last_sample_when_the_reply_is_full() {
        let own = addr(3000);
        let sampled = (1..=10).map(addr).collect();
        let peers = include_own_address(sampled, Some(own), 20, addr(9));
        assert_eq!(peers.len(), usize::from(SHARE_POLICY_MAX));
        assert_eq!(peers.last().copied(), Some(own));
        assert!(!peers.contains(&addr(10)));
        assert!(peers.contains(&addr(1)));
    }

    #[test]
    fn own_address_is_omitted_when_absent_unspecified_or_the_requester() {
        let sampled = vec![addr(1)];
        assert_eq!(include_own_address(sampled.clone(), None, 10, addr(9)), sampled);
        let wildcard = SocketAddr::from(([0, 0, 0, 0], 3000));
        assert_eq!(include_own_address(sampled.clone(), Some(wildcard), 10, addr(9)), sampled);
        let requester = addr(9);
        assert_eq!(include_own_address(sampled.clone(), Some(requester), 10, requester), sampled);
        assert_eq!(include_own_address(vec![addr(1)], Some(addr(3000)), 0, addr(9)), Vec::<SocketAddr>::new());
    }

    #[test]
    fn own_address_already_in_the_sample_is_not_repeated() {
        let own = addr(3000);
        let peers = include_own_address(vec![addr(1), own], Some(own), 10, addr(9));
        assert_eq!(peers, vec![addr(1), own]);
    }
}
