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

//! Peer-sharing mini-protocol (node-to-node protocol number 10).
//!
//! Simple non-pipelined request/reply (network-spec §3.11):
//! - client: `MsgShareRequest(amount)` → wait → `MsgSharePeers`
//! - server: wait → `MsgShareRequest` → `MsgSharePeers`
//!
//! The initiator runs on an established connection's maintenance group when the remote side is
//! advertisable, and writes each reply to the peer-tracking resource. The responder reads its
//! reply from that resource and appends this node's own listen address when it has one.

mod initiator;
mod messages;
mod responder;

use std::time::Duration;

use amaru_kernel::Peer;
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef};
pub use initiator::{PeerSharingInitiator, PeerSharingMessage, ShareResult, initiator};

/// Delay after the maintenance group starts before the first share request.
pub const SHARE_REQUEST_INITIAL_DELAY: Duration = Duration::from_secs(300);
/// Interval between a share reply and the next request.
pub const SHARE_REQUEST_INTERVAL: Duration = Duration::from_secs(900);
/// How long the client may stay busy waiting for `MsgSharePeers`.
///
/// This is the `StBusy` limit. Exceeding it ends the mini-protocol and therefore the bearer.
/// The client does not return to idle and does not send another `MsgShareRequest`.
pub const SHARE_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// How many peers to request per share call (network-spec amount is `Word8`).
pub const SHARE_REQUEST_AMOUNT: u8 = 20;
/// Upper bound on addresses in one share reply. The sampler and the responder both use it.
pub const SHARE_POLICY_MAX: u8 = 10;
pub use messages::{MAX_MESSAGE_BYTES, Message};
pub use responder::{PeerSharingResponder, register_peer_sharing_responder, responder};

use crate::{
    mux::{Frame, MuxMessage},
    protocol::{PROTO_N2N_PEER_SHARE, ProtoSpec, ProtocolState, RoleT, ingress_limit},
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![initiator::register_deserializers(), responder::register_deserializers()].into_iter().flatten().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum State {
    Idle,
    Busy,
    Done,
}

pub fn spec<R: RoleT>() -> ProtoSpec<State, Message, R>
where
    State: ProtocolState<R, WireMsg = Message>,
{
    let mut spec = ProtoSpec::default();
    let share_request = || Message::ShareRequest { amount: 1 };
    let share_peers = || Message::SharePeers { peers: Vec::new() };
    let done = || Message::Done;

    // Client sends ShareRequest from Idle → Busy
    spec.init(State::Idle, share_request(), State::Busy);
    // Client may send Done from Idle → Done
    if R::ROLE == Some(crate::protocol::Role::Initiator) {
        spec.init(State::Idle, done(), State::Done);
    } else {
        spec.init(State::Idle, done(), State::Idle);
    }
    // Server replies SharePeers from Busy → Idle
    spec.resp(State::Busy, share_peers(), State::Idle);

    spec
}

/// Register the peer-sharing **initiator** (client) on the mux.
#[expect(clippy::too_many_arguments)]
pub async fn register_peer_sharing_initiator<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    amount: u8,
    initial_delay: Duration,
    interval: Duration,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<PeerSharingMessage> {
    use crate::protocol::Inputs;

    let (state, stage) = PeerSharingInitiator::new(muxer.clone(), peer, conn_id, amount, initial_delay, interval);
    let ps = eff.stage("peer_sharing", initiator()).await;
    let ps = eff.supervise(ps, tombstone);
    let ps = eff.wire_up(ps, (state, stage)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_PEER_SHARE.erase(),
            frame: Frame::OneCborItem,
            handler: ps.contramap(Inputs::<PeerSharingMessage>::Network),
            max_buffer: ingress_limit(PROTO_N2N_PEER_SHARE),
        },
    )
    .await;
    ps.contramap(Inputs::<PeerSharingMessage>::Local)
}

/// Register a one-shot peer-sharing initiator for probes: one request, one [`ShareResult`], then Done.
pub async fn register_peer_sharing_initiator_oneshot<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    amount: u8,
    initial_delay: Duration,
    report_to: StageRef<ShareResult>,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<PeerSharingMessage> {
    use crate::protocol::Inputs;

    let (state, stage) = PeerSharingInitiator::oneshot(muxer.clone(), peer, conn_id, amount, initial_delay, report_to);
    let ps = eff.stage("peer_sharing", initiator()).await;
    let ps = eff.supervise(ps, tombstone);
    let ps = eff.wire_up(ps, (state, stage)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_PEER_SHARE.erase(),
            frame: Frame::OneCborItem,
            handler: ps.contramap(Inputs::<PeerSharingMessage>::Network),
            max_buffer: ingress_limit(PROTO_N2N_PEER_SHARE),
        },
    )
    .await;
    ps.contramap(Inputs::<PeerSharingMessage>::Local)
}
