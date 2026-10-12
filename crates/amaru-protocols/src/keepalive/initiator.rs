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

use std::time::Duration;

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug, debug_span};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, Instant, StageRef, Void};

use crate::{
    keepalive::{
        State,
        messages::{Cookie, Message},
    },
    mux::MuxMessage,
    peer_tracking_effects::PeerTrack,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_KEEP_ALIVE, ProtocolState, StageState, miniprotocol,
        outcome,
    },
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<InitiatorMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(State, KeepAliveInitiator)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<KeepAliveInitiator>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, KeepAliveInitiator, Initiator> {
    miniprotocol(PROTO_N2N_KEEP_ALIVE)
}

/// Message sent to the handler to trigger periodic keep-alive sends
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorMessage {
    SendKeepAlive,
    Close,
}

/// Cookie echoed by the remote keep-alive response.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InitiatorResult {
    pub cookie: Cookie,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeepAliveInitiator {
    cookie: Cookie,
    peer: Peer,
    conn_id: ConnectionId,
    sent_at: Option<(Cookie, Instant)>,
    muxer: StageRef<MuxMessage>,
    pending_close: bool,
}

impl KeepAliveInitiator {
    pub fn new(peer: Peer, conn_id: ConnectionId, muxer: StageRef<MuxMessage>) -> (State, Self) {
        (State::Idle, Self { cookie: Cookie::new(), peer, conn_id, sent_at: None, muxer, pending_close: false })
    }
}

impl StageState<State, Initiator> for KeepAliveInitiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        mut self,
        proto: &State,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        use State::*;

        match (proto, input) {
            (Idle, InitiatorMessage::SendKeepAlive) if !self.pending_close => {
                self.sent_at = Some((self.cookie, eff.clock().await));
                Ok((Some(InitiatorAction::SendKeepAlive(self.cookie)), self))
            }
            (Idle, InitiatorMessage::SendKeepAlive) => Ok((Some(InitiatorAction::Done), self)),
            (Idle, InitiatorMessage::Close) => Ok((Some(InitiatorAction::Done), self)),
            (Waiting, InitiatorMessage::Close) => {
                self.pending_close = true;
                Ok((None, self))
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        }
    }

    async fn network(
        mut self,
        _proto: &State,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        // After receiving a response, increment cookie and schedule next send
        let cookie = input.cookie.as_u16();

        async move {
            let received_at = eff.clock().await;
            if let Some((sent_cookie, sent_at)) = self.sent_at.take()
                && sent_cookie == input.cookie
            {
                let round_trip = received_at.saturating_since(sent_at);
                let round_trip_micros = round_trip.as_micros() as u64;
                debug!(
                    protocols::keepalive::peer::ROUND_TRIP,
                    peer = &self.peer,
                    conn_id = self.conn_id.as_u64(),
                    round_trip_micros
                );
                PeerTrack::new(eff).record_keepalive_rtt(self.peer, round_trip, received_at).await;
            }
            self.cookie = input.cookie.next();
            if self.pending_close {
                return Ok((Some(InitiatorAction::Done), self));
            }
            let delay = if u16::from(input.cookie) == 0 {
                // this is only for the very first keep-alive message, which the Haskell node expects within the first
                // five seconds
                Duration::from_secs(1)
            } else {
                Duration::from_secs(30)
            };
            eff.schedule_after(Inputs::Local(InitiatorMessage::SendKeepAlive), delay).await;
            Ok((None, self))
        }
        .instrument(debug_span!(protocols::keepalive::initiator::KEEPALIVE_INITIATOR_STAGE, cookie))
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
        // On init, trigger the first KeepAlive send via the StageState to set timers in motion
        Ok((outcome().result(InitiatorResult { cookie: Cookie::new() }), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = debug_span!(
            protocols::keepalive::initiator::KEEPALIVE_INITIATOR_PROTOCOL,
            message_type = input.message_type().to_string()
        );
        let _guard = _span.enter();
        use State::*;

        Ok(match (self, input) {
            (Waiting, Message::ResponseKeepAlive(cookie)) => (outcome().result(InitiatorResult { cookie }), Idle),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Idle, InitiatorAction::SendKeepAlive(cookie)) => {
                (outcome().send(Message::KeepAlive(cookie)).want_next(), Waiting)
            }
            (Idle, InitiatorAction::Done) => (outcome().send(Message::Done).finish(), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum InitiatorAction {
    SendKeepAlive(Cookie),
    Done,
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, OnceLock};

    use amaru_kernel::{NonEmptyBytes, cbor};
    use amaru_ouroboros::{ConnectionId, PeerTrackingResource};
    use amaru_pure_stage::{
        StageGraph, assert_trace_contains,
        simulation::{Run, SimulationBuilder},
        tm_external_effect_any,
        trace_buffer::TraceBuffer,
    };
    use tokio::runtime::{Builder, Runtime};

    use super::*;
    use crate::{
        keepalive::{State, initiator::InitiatorAction, messages::Message},
        mux::{HandlerMessage, MuxMessage, Sent},
        peer_tracking::InMemoryPeerTracking,
        peer_tracking_effects::RecordKeepaliveRttEffect,
        protocol::Initiator,
    };

    #[test]
    fn test_initiator_protocol() {
        crate::keepalive::spec::<Initiator>().check(State::Idle, |msg| match msg {
            Message::KeepAlive(cookie) => Some(InitiatorAction::SendKeepAlive(*cookie)),
            Message::Done => Some(InitiatorAction::Done),
            Message::ResponseKeepAlive(_) => None,
        });
    }

    fn test_runtime() -> &'static tokio::runtime::Handle {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| Builder::new_current_thread().enable_all().build().expect("runtime")).handle()
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
    struct MuxLog {
        cookie: Option<Cookie>,
    }

    async fn mux_step(mut log: MuxLog, msg: MuxMessage, eff: Effects<MuxMessage>) -> MuxLog {
        if let MuxMessage::Send(_, bytes, cr) = msg {
            let decoded: Message = cbor::decode(bytes.as_ref()).expect("cbor");
            if let Message::KeepAlive(cookie) = decoded {
                log.cookie = Some(cookie);
            }
            eff.send(&cr, Sent).await;
        }
        log
    }

    #[test]
    fn a_matching_response_records_the_round_trip() {
        let _guards = (register_deserializers(), crate::peer_tracking_effects::register_deserializers());
        let _effect = amaru_pure_stage::register_effect_deserializer::<RecordKeepaliveRttEffect>();
        let tracking: PeerTrackingResource = Arc::new(InMemoryPeerTracking::new());
        let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000));
        network.resources().put::<PeerTrackingResource>(tracking);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let peer = Peer::for_test(3001);
        let (proto, stage) = KeepAliveInitiator::new(peer, ConnectionId::initial(), mux_ref);
        let built = network.stage("keepalive", initiator());
        let keepalive = network.wire_up(built, (proto, stage));
        network
            .preload(&keepalive, [Inputs::Network(HandlerMessage::Registered(PROTO_N2N_KEEP_ALIVE.erase()))])
            .expect("preload");
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        let started = running.now();
        running.run(Run::until(started + Duration::from_secs(1))).assert_idle();
        let cookie = running.get_state(&mux).and_then(|log| log.cookie).expect("keepalive sent");
        running.enqueue_msg(
            &keepalive,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::ResponseKeepAlive(cookie))))],
        );
        running.run(Run::default()).assert_sleeping();
        assert_trace_contains(&running, &[tm_external_effect_any::<RecordKeepaliveRttEffect>()]);
    }
}
