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

//! Probe / diagnostic keep-alive initiator: caller-driven sends, RTT reported to a sink.
//!
//! Kept separate from the node [`super::initiator`] so the production periodic handler can stay
//! unchanged until it is rewritten in the newer typestate style.

use std::time::Duration;

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug, debug_span};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, Instant, StageRef, Void};

use crate::{
    keepalive::{
        initiator::InitiatorMessage,
        messages::{Cookie, Message},
    },
    mux::MuxMessage,
    peer_tracking_effects::PeerTrack,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_KEEP_ALIVE, ProtocolState, StageState, miniprotocol,
        outcome,
    },
};

/// Wire / local protocol states for the oneshot initiator (mirrors node keepalive states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum OneshotState {
    Idle,
    Waiting,
    Done,
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<OneshotResult>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(OneshotState, KeepAliveOneshotInitiator)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<KeepAliveOneshotInitiator>().boxed(),
    ]
}

pub fn oneshot_initiator() -> Miniprotocol<OneshotState, KeepAliveOneshotInitiator, Initiator> {
    miniprotocol(PROTO_N2N_KEEP_ALIVE)
}

/// Outcomes from the oneshot protocol machine to [`StageState`].
///
/// [`OneshotResult::Bootstrap`] is synthetic (`ProtocolState::init`) and must never be treated as a
/// wire `ResponseKeepAlive` — doing so races with a pending send and reports a zero RTT.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum OneshotResult {
    Bootstrap,
    /// A `ResponseKeepAlive` was observed. `round_trip` is filled when a matching send was outstanding.
    Response {
        cookie: Cookie,
        round_trip: Option<Duration>,
    },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeepAliveOneshotInitiator {
    cookie: Cookie,
    peer: Peer,
    conn_id: ConnectionId,
    sent_at: Option<(Cookie, Instant)>,
    muxer: StageRef<MuxMessage>,
    pending_close: bool,
    /// Sink for measured RTTs (e.g. `amaru peer probe --ping`).
    report_to: StageRef<OneshotResult>,
}

impl KeepAliveOneshotInitiator {
    pub fn new(
        peer: Peer,
        conn_id: ConnectionId,
        muxer: StageRef<MuxMessage>,
        report_to: StageRef<OneshotResult>,
    ) -> (OneshotState, Self) {
        (
            OneshotState::Idle,
            Self { cookie: Cookie::new(), peer, conn_id, sent_at: None, muxer, pending_close: false, report_to },
        )
    }
}

impl StageState<OneshotState, Initiator> for KeepAliveOneshotInitiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        mut self,
        proto: &OneshotState,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<OneshotAction>, Self)> {
        use OneshotState::*;

        match (proto, input) {
            (Idle, InitiatorMessage::SendKeepAlive) if !self.pending_close => {
                self.sent_at = Some((self.cookie, eff.clock().await));
                Ok((Some(OneshotAction::SendKeepAlive(self.cookie)), self))
            }
            (Idle, InitiatorMessage::SendKeepAlive) => Ok((Some(OneshotAction::Done), self)),
            (Idle, InitiatorMessage::Close) => Ok((Some(OneshotAction::Done), self)),
            (Waiting, InitiatorMessage::Close) => {
                self.pending_close = true;
                Ok((None, self))
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        }
    }

    async fn network(
        mut self,
        _proto: &OneshotState,
        input: OneshotResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<OneshotAction>, Self)> {
        match input {
            OneshotResult::Bootstrap => Ok((None, self)),
            OneshotResult::Response { cookie, .. } => {
                let cookie_u16 = cookie.as_u16();
                async move {
                    let received_at = eff.clock().await;
                    if let Some((sent_cookie, sent_at)) = self.sent_at.take()
                        && sent_cookie == cookie
                    {
                        let rtt = received_at.saturating_since(sent_at);
                        let round_trip_micros = rtt.as_micros() as u64;
                        debug!(
                            protocols::keepalive::peer::ROUND_TRIP,
                            peer = &self.peer,
                            conn_id = self.conn_id.as_u64(),
                            round_trip_micros
                        );
                        PeerTrack::new(eff).record_keepalive_rtt(self.peer, rtt, received_at).await;
                        eff.send(&self.report_to, OneshotResult::Response { cookie, round_trip: Some(rtt) }).await;
                    }
                    self.cookie = cookie.next();
                    if self.pending_close {
                        return Ok((Some(OneshotAction::Done), self));
                    }
                    Ok((None, self))
                }
                .instrument(
                    debug_span!(protocols::keepalive::initiator::KEEPALIVE_INITIATOR_STAGE, cookie = cookie_u16).into(),
                )
                .await
            }
        }
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Initiator> for OneshotState {
    type WireMsg = Message;
    type Action = OneshotAction;
    type Out = OneshotResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().result(OneshotResult::Bootstrap), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = debug_span!(
            protocols::keepalive::initiator::KEEPALIVE_INITIATOR_PROTOCOL,
            message_type = input.message_type().to_string()
        );
        let _guard = _span.enter();
        use OneshotState::*;

        Ok(match (self, input) {
            (Waiting, Message::ResponseKeepAlive(cookie)) => {
                (outcome().result(OneshotResult::Response { cookie, round_trip: None }), Idle)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use OneshotState::*;

        Ok(match (self, input) {
            (Idle, OneshotAction::SendKeepAlive(cookie)) => {
                (outcome().send(Message::KeepAlive(cookie)).want_next(), Waiting)
            }
            (Idle, OneshotAction::Done) => (outcome().send(Message::Done).finish(), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum OneshotAction {
    SendKeepAlive(Cookie),
    Done,
}
