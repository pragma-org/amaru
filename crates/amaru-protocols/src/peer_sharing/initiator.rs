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

//! Peer-sharing initiator (client): non-pipelined request/response with its own cadence.
//!
//! The connection starts this stage with the maintenance group when the remote side is
//! advertisable. One timer waits `initial_delay` before the first request. The request is sent
//! only from the idle state. It moves the mini-protocol to busy, which is server agency, and arms
//! [`SHARE_REQUEST_TIMEOUT`]. That timeout is the busy-state limit: exceeding it ends this
//! mini-protocol. The connection stage treats that death as unexpected and tears the bearer down.
//! The stage does not return to idle and does not send another request or `MsgDone` from busy.
//! A reply is a class C9 write, so this stage awaits the resource. After a reply it waits
//! `interval` before the next request. An idle close cancels the timer.

use std::{net::SocketAddr, time::Duration};

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug, debug_span, info, warn};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, ScheduleId, StageRef, Void};

use crate::{
    mux::MuxMessage,
    peer_sharing::{SHARE_REQUEST_TIMEOUT, State, messages::Message},
    peer_tracking_effects::PeerTrack,
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
        amaru_pure_stage::register_data_deserializer::<Inputs<PeerSharingMessage>>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, PeerSharingInitiator, Initiator> {
    miniprotocol(PROTO_N2N_PEER_SHARE)
}

/// Local messages into the peer-sharing initiator stage.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSharingMessage {
    /// Internal timer: send the next share request if idle.
    Tick,
    /// Clean shutdown: `MsgDone` when Idle, otherwise wait for the in-flight reply.
    Close,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSharingInitiator {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    /// Request size, fixed for this connection.
    amount: u8,
    /// Delay before the first request.
    initial_delay: Duration,
    /// Delay between a reply and the next request.
    interval: Duration,
    /// Outstanding timer for the next [`PeerSharingMessage::Tick`].
    timer: Option<ScheduleId>,
    /// True while waiting for `MsgSharePeers`.
    in_flight: bool,
    /// Close arrived while a request was in flight.
    closing: bool,
}

impl PeerSharingInitiator {
    pub fn new(
        muxer: StageRef<MuxMessage>,
        peer: Peer,
        conn_id: ConnectionId,
        amount: u8,
        initial_delay: Duration,
        interval: Duration,
    ) -> (State, Self) {
        (
            State::Idle,
            Self {
                muxer,
                peer,
                conn_id,
                amount,
                initial_delay,
                interval,
                timer: None,
                in_flight: false,
                closing: false,
            },
        )
    }

    async fn arm_timer(&mut self, delay: Duration, eff: &Effects<Inputs<PeerSharingMessage>>) -> anyhow::Result<()> {
        if let Some(old) = self.timer.take() {
            eff.cancel_schedule(old).await;
        }
        self.timer = Some(eff.schedule_after(Inputs::Local(PeerSharingMessage::Tick), delay).await);
        Ok(())
    }
}

impl StageState<State, Initiator> for PeerSharingInitiator {
    type LocalIn = PeerSharingMessage;

    async fn local(
        mut self,
        proto: &State,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        match input {
            PeerSharingMessage::Tick => {
                self.timer = None;
                if self.in_flight {
                    // StBusy limit, including after Close. The stage ends; the connection tears the bearer down.
                    debug!(
                        protocols::peer_sharing::initiator::REQUEST_TIMEOUT,
                        peer = self.peer,
                        conn_id = self.conn_id.as_u64()
                    );
                    return eff.terminate().await;
                }
                if self.closing {
                    return Ok((None, self));
                }
                match proto {
                    State::Idle => {
                        self.in_flight = true;
                        let amount = self.amount;
                        self.arm_timer(SHARE_REQUEST_TIMEOUT, eff).await?;
                        Ok((Some(InitiatorAction::ShareRequest { amount }), self))
                    }
                    State::Busy | State::Done => Ok((None, self)),
                }
            }
            PeerSharingMessage::Close => match proto {
                State::Idle if !self.in_flight => {
                    if let Some(id) = self.timer.take() {
                        eff.cancel_schedule(id).await;
                    }
                    Ok((Some(InitiatorAction::Done), self))
                }
                State::Busy | State::Idle => {
                    self.closing = true;
                    Ok((None, self))
                }
                State::Done => Ok((None, self)),
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
            peer = &self.peer,
            conn_id = self.conn_id.as_u64()
        );
        async move {
            match input {
                InitiatorResult::Started => {
                    self.arm_timer(self.initial_delay, eff).await?;
                    Ok((None, self))
                }
                InitiatorResult::SharePeers { peers } => {
                    if !self.in_flight {
                        warn!(protocols::peer_sharing::initiator::PROTOCOL_VIOLATION, reason = "no_request_in_flight");
                        return eff.terminate().await;
                    }
                    if self.closing {
                        self.in_flight = false;
                        return Ok((Some(InitiatorAction::Done), self));
                    }
                    if peers.len() > self.amount as usize {
                        warn!(
                            protocols::peer_sharing::initiator::PROTOCOL_VIOLATION,
                            reason = "too_many_addresses",
                            requested = self.amount,
                            received = peers.len()
                        );
                        return eff.terminate().await;
                    }
                    self.in_flight = false;
                    let now = eff.clock().await;
                    let peers_list = peers.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                    let recorded = PeerTrack::new(eff).record_shared_peers(self.peer, peers, now).await;
                    info!(
                        protocols::peer_selection::sharing::RECEIVED,
                        peer = self.peer,
                        peers = peers_list,
                        added = recorded.added,
                        total = recorded.total,
                    );
                    // Next request after the configured interval. Replaces the reply timeout.
                    self.arm_timer(self.interval, eff).await?;
                    Ok((None, self))
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
        // Arm the first-request timer. No wire message until that timer fires.
        Ok((outcome().result(InitiatorResult::Started), *self))
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
            (Idle, InitiatorAction::Done) => (outcome().send(Message::Done).finish(), Done),
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
    /// Protocol start: arm the first-request timer. Not a wire message.
    Started,
    SharePeers {
        peers: Vec<SocketAddr>,
    },
}

#[cfg(test)]
pub mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex, OnceLock},
    };

    use amaru_kernel::cbor;
    use amaru_ouroboros::ConnectionId;
    use amaru_pure_stage::{
        Effect, StageGraph,
        simulation::{Run, SimulationBuilder, SimulationRunning},
        trace_buffer::{TraceBuffer, TraceEntry},
    };
    use tokio::runtime::{Builder, Runtime};
    use tracing_subscriber::util::SubscriberInitExt;

    use super::*;
    use crate::{
        mux::{HandlerMessage, MuxMessage, Sent},
        peer_sharing::SHARE_REQUEST_INTERVAL,
        protocol::Initiator,
    };

    #[test]
    fn test_initiator_protocol() {
        crate::peer_sharing::spec::<Initiator>().check(State::Idle, |msg| match msg {
            Message::ShareRequest { amount } => Some(InitiatorAction::ShareRequest { amount: *amount }),
            Message::Done => Some(InitiatorAction::Done),
            Message::SharePeers { .. } => None,
        });
    }

    #[derive(Clone)]
    struct LogBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for LogBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("log lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_runtime() -> &'static tokio::runtime::Handle {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| Builder::new_current_thread().enable_all().build().expect("runtime")).handle()
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
    struct MuxLog {
        sends: Vec<String>,
    }

    async fn mux_step(mut log: MuxLog, msg: MuxMessage, eff: Effects<MuxMessage>) -> MuxLog {
        match msg {
            MuxMessage::Send(_, bytes, cr) => {
                let decoded: Message = cbor::decode(bytes.as_ref()).expect("cbor");
                log.sends.push(decoded.message_type().to_string());
                eff.send(&cr, Sent).await;
            }
            MuxMessage::WantNext(_)
            | MuxMessage::Register { .. }
            | MuxMessage::Buffer(..)
            | MuxMessage::FromNetwork(..)
            | MuxMessage::Written
            | MuxMessage::Terminate
            | MuxMessage::SetSduTimeout(_) => {}
        }
        log
    }

    fn suspends(running: &SimulationRunning) -> Vec<Effect> {
        running
            .trace_buffer()
            .lock()
            .iter_entries()
            .filter_map(|(_, entry)| if let TraceEntry::Suspend(effect) = entry { Some(effect) } else { None })
            .collect()
    }

    fn schedule_deadlines(running: &SimulationRunning) -> Vec<amaru_pure_stage::Instant> {
        suspends(running)
            .into_iter()
            .filter_map(|effect| if let Effect::Schedule { id, .. } = effect { Some(id.time()) } else { None })
            .collect()
    }

    fn initiator_state(running: &SimulationRunning) -> (State, PeerSharingInitiator) {
        let mut found = None;
        for (_, entry) in running.trace_buffer().lock().iter_entries() {
            if let TraceEntry::State { stage, state } = entry
                && stage.as_str().starts_with("peer_sharing")
                && let Ok(state) = state.cast::<(State, PeerSharingInitiator)>()
            {
                found = Some(*state);
            }
        }
        found.expect("peer-sharing initiator state")
    }

    fn start_initiator(
        initial_delay: Duration,
        interval: Duration,
    ) -> (SimulationRunning, amaru_pure_stage::StageRef<Inputs<PeerSharingMessage>>) {
        let peer = Peer::for_test(3001);
        let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000));
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let _mux = network.wire_up(mux, MuxLog::default());
        let (proto, stage) =
            PeerSharingInitiator::new(mux_ref, peer, ConnectionId::initial(), 20, initial_delay, interval);
        let built = network.stage("peer_sharing", initiator());
        let ps = network.wire_up(built, (proto, stage));
        network
            .preload(&ps, [Inputs::Network(HandlerMessage::Registered(PROTO_N2N_PEER_SHARE.erase()))])
            .expect("preload");
        (network.run(test_runtime()), ps.without_state())
    }

    fn mux_sends(running: &SimulationRunning) -> Vec<String> {
        let mut found = Vec::new();
        for (_, entry) in running.trace_buffer().lock().iter_entries() {
            if let TraceEntry::State { stage, state } = entry
                && stage.as_str().starts_with("mux")
                && let Ok(state) = state.cast::<MuxLog>()
            {
                found.clone_from(&state.sends);
            }
        }
        found
    }

    #[test]
    fn a_missing_share_reply_terminates_without_another_request() {
        let logs = LogBuf(Arc::new(Mutex::new(Vec::new())));
        let _guard = tracing_subscriber::fmt()
            .with_max_level(amaru_observability::tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer({
                let logs = logs.clone();
                move || logs.clone()
            })
            .set_default();

        let _guards = register_deserializers();
        let _mux_state = amaru_pure_stage::register_data_deserializer::<MuxLog>();
        let (mut running, _ps) = start_initiator(Duration::from_secs(1), SHARE_REQUEST_INTERVAL);
        running.run(Run::default()).assert_sleeping();
        let started = running.now();
        running.run(Run::until(started + Duration::from_secs(1))).assert_sleeping();
        let asked_at = started + Duration::from_secs(1);
        assert_eq!(
            schedule_deadlines(&running),
            vec![asked_at, asked_at + SHARE_REQUEST_TIMEOUT],
            "the request arms one reply timeout"
        );
        let (proto, stage) = initiator_state(&running);
        assert_eq!(proto, State::Busy);
        assert!(stage.in_flight);
        assert_eq!(mux_sends(&running), vec!["ShareRequest".to_string()]);

        // This simulation's initiator is top-level, so terminate ends the run. In the node the
        // connection stage supervises it. `ChildDied` for peer-sharing is not in `stopping`, and
        // that arm of `connection.rs` tears the bearer down.
        running.run(Run::until(asked_at + SHARE_REQUEST_TIMEOUT)).assert_terminated("peer_sharing");
        assert_eq!(mux_sends(&running), vec!["ShareRequest".to_string()], "no second request and no MsgDone");
        let repeat_at = asked_at + SHARE_REQUEST_TIMEOUT + SHARE_REQUEST_INTERVAL;
        assert!(!schedule_deadlines(&running).contains(&repeat_at), "the timeout does not arm another request");
        let (proto, _) = initiator_state(&running);
        assert_eq!(proto, State::Busy, "the timeout does not return the protocol to idle");
        let text = String::from_utf8(logs.0.lock().expect("log lock").clone()).expect("utf-8");
        assert!(text.contains("request_timeout"), "{text}");
    }

    #[test]
    fn a_close_during_an_inflight_request_still_times_out() {
        let _guards = register_deserializers();
        let _mux_state = amaru_pure_stage::register_data_deserializer::<MuxLog>();
        let (mut running, ps) = start_initiator(Duration::from_secs(1), SHARE_REQUEST_INTERVAL);
        running.run(Run::default()).assert_sleeping();
        let started = running.now();
        running.run(Run::until(started + Duration::from_secs(1))).assert_sleeping();
        let asked_at = started + Duration::from_secs(1);
        let (proto, stage) = initiator_state(&running);
        assert_eq!(proto, State::Busy);
        assert!(stage.in_flight);

        running.enqueue_msg(&ps, [Inputs::Local(PeerSharingMessage::Close)]);
        running.run(Run::until(asked_at + SHARE_REQUEST_TIMEOUT)).assert_terminated("peer_sharing");
        assert_eq!(mux_sends(&running), vec!["ShareRequest".to_string()], "no MsgDone from StBusy");
    }

    #[test]
    fn an_idle_close_cancels_the_share_timer() {
        let _guards = register_deserializers();
        let (mut running, ps) = start_initiator(Duration::from_secs(300), SHARE_REQUEST_INTERVAL);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(schedule_deadlines(&running).len(), 1);
        running.enqueue_msg(&ps, [Inputs::Local(PeerSharingMessage::Close)]);
        let _ = running.run(Run::default());
        let cancels =
            suspends(&running).into_iter().filter(|effect| matches!(effect, Effect::CancelSchedule { .. })).count();
        // Close terminates the stage before another state snapshot, so the cancel in the trace is the record.
        assert_eq!(cancels, 1, "the idle close drops the outstanding timer");
    }
}
