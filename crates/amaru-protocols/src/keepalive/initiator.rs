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

/// Message sent from the handler (for future use, e.g., RTT reporting)
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
                let round_trip_micros = received_at.saturating_since(sent_at).as_micros() as u64;
                debug!(
                    protocols::keepalive::peer::ROUND_TRIP,
                    peer = &self.peer,
                    conn_id = self.conn_id.as_u64(),
                    round_trip_micros
                );
            }
            self.cookie = input.cookie.next();
            if self.pending_close {
                return Ok((Some(InitiatorAction::Done), self));
            }
            let delay = if u16::from(input.cookie) == 0 {
                // this is only for the very first keep-alive message, which the Haskell node expects within the first
                // five seconds
                super::KEEPALIVE_FIRST_DELAY
            } else {
                super::KEEPALIVE_INTERVAL
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
    use crate::{
        keepalive::{State, initiator::InitiatorAction, messages::Message},
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

    use std::time::Duration;

    use amaru_kernel::{NonEmptyBytes, Peer};
    use amaru_ouroboros::ConnectionId;
    use amaru_pure_stage::{
        Effect, StageGraph, StageRef,
        simulation::{Blocked, Run, SimulationBuilder},
        trace_buffer::{TraceBuffer, TraceEntry},
    };

    use super::{InitiatorMessage, KeepAliveInitiator, initiator};
    use crate::{
        keepalive::messages::Cookie,
        mux::{MuxMessage, Sent},
        protocol::{Inputs, egress_admission_deadline},
    };

    async fn hold(_state: u8, msg: MuxMessage, eff: amaru_pure_stage::Effects<MuxMessage>) -> u8 {
        if let MuxMessage::Send(_, _, _) = msg {
            eff.wait(Duration::from_secs(3600)).await;
        }
        0
    }

    async fn reply(_state: u8, msg: MuxMessage, eff: amaru_pure_stage::Effects<MuxMessage>) -> u8 {
        if let MuxMessage::Send(_, _, cr) = msg {
            eff.send(&cr, Sent).await;
        }
        0
    }

    fn keepalive_bytes() -> usize {
        NonEmptyBytes::encode(&Message::KeepAlive(Cookie::new())).len().get()
    }

    #[test]
    fn egress_timeout_faults_before_want_next_and_leaves_the_protocol_state() {
        let _guards = crate::mux::register_deserializers();
        let _ka = crate::keepalive::register_deserializers();
        let trace = TraceBuffer::new_shared(200, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace);
        let mux = network.stage("mux", hold);
        let mux = network.wire_up(mux, 0u8);
        let handler_b = network.stage("ka", initiator());
        let handler = network.wire_up(
            handler_b,
            KeepAliveInitiator::new(Peer::for_test(3007), ConnectionId::initial(), StageRef::clone(&mux)),
        );
        network.preload(&handler, [Inputs::Local(InitiatorMessage::SendKeepAlive)]).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        let deadline = egress_admission_deadline(keepalive_bytes());
        let start = running.now();
        let early = running.run(Run::until(start + deadline - Duration::from_millis(1)));
        assert!(matches!(early, Blocked::Sleeping { .. }), "deadline has not fired, got {early:?}");
        let _alive = running.mailbox_len(&handler);
        let blocked = running.run(Run::until(start + deadline));
        assert!(matches!(blocked, Blocked::Terminated(ref name) if name == handler.name()), "{blocked:?}");
        let entries: Vec<TraceEntry> = running.trace_buffer().lock().iter_entries().map(|(_, e)| e).collect();
        assert!(entries.iter().all(|entry| {
            !matches!(
                entry,
                TraceEntry::Suspend(Effect::Send { from, msg, .. })
                    if from == handler.name()
                        && msg.cast_ref::<MuxMessage>().is_ok_and(|m| matches!(m, MuxMessage::WantNext(_)))
            )
        }));
        for entry in &entries {
            let TraceEntry::State { stage, state } = entry else { continue };
            if stage != handler.name() {
                continue;
            }
            let (proto, _) = state.cast_ref::<(State, KeepAliveInitiator)>().expect("keepalive state");
            assert_eq!(*proto, State::Idle);
        }
    }

    #[test]
    fn accepted_egress_sends_want_next_after_the_payload() {
        let _guards = crate::mux::register_deserializers();
        let _ka = crate::keepalive::register_deserializers();
        let trace = TraceBuffer::new_shared(200, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace);
        let mux = network.stage("mux", reply);
        let mux = network.wire_up(mux, 0u8);
        let handler_b = network.stage("ka", initiator());
        let handler = network.wire_up(
            handler_b,
            KeepAliveInitiator::new(Peer::for_test(3007), ConnectionId::initial(), StageRef::clone(&mux)),
        );
        network.preload(&handler, [Inputs::Local(InitiatorMessage::SendKeepAlive)]).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::skip_wakeups()).assert_idle();
        let state = running.get_state(&handler).expect("handler");
        assert_eq!(state.0, State::Waiting);
        let entries: Vec<TraceEntry> = running.trace_buffer().lock().iter_entries().map(|(_, e)| e).collect();
        let call_at = entries.iter().position(|entry| match entry {
            TraceEntry::Suspend(Effect::Call { from, duration, msg, .. }) if from == handler.name() => {
                let Ok(MuxMessage::Send(_, bytes, _)) = msg.cast_ref::<MuxMessage>() else {
                    return false;
                };
                bytes.as_ref() == NonEmptyBytes::encode(&Message::KeepAlive(Cookie::new())).as_ref()
                    && *duration == egress_admission_deadline(bytes.len().get())
            }
            TraceEntry::Suspend(_)
            | TraceEntry::Resume { .. }
            | TraceEntry::Clock(_)
            | TraceEntry::Input { .. }
            | TraceEntry::State { .. }
            | TraceEntry::Terminated { .. }
            | TraceEntry::InvalidBytes(..) => false,
        });
        let want_at = entries.iter().position(|entry| {
            matches!(
                entry,
                TraceEntry::Suspend(Effect::Send { from, msg, .. })
                    if from == handler.name()
                        && msg.cast_ref::<MuxMessage>().is_ok_and(|m| matches!(m, MuxMessage::WantNext(_)))
            )
        });
        let (call_at, want_at) = (call_at.expect("call"), want_at.expect("WantNext"));
        assert!(call_at < want_at, "WantNext follows an accepted send");
    }
}
