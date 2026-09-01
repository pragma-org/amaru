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

//! Peer-sharing initiator (client): non-pipelined request/response with internal cadence.
//!
//! Lifecycle follows the connection: once [`PeerSharingMessage::Start`] is received, the stage
//! schedules itself for the first request after `initial_delay`, then again after each successful
//! reply using `interval`. Timers die with the stage when the connection is torn down.

use std::{net::SocketAddr, time::Duration};

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug_span, warn};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, ScheduleId, StageRef, Void};

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

/// Local messages into the peer-sharing initiator stage.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSharingMessage {
    /// Begin (or restart) periodic share requests for this connection.
    ///
    /// - First network request after `initial_delay`.
    /// - Further requests after each reply, delayed by `interval`.
    /// - `reply_to` is used for every result until the protocol stage ends.
    Start { amount: u8, initial_delay: Duration, interval: Duration, reply_to: StageRef<ShareResult> },
    /// Internal timer: send the next share request if idle.
    Tick,
}

/// Reply delivered to the requester after `MsgSharePeers`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShareResult {
    /// Peer that answered the share request (the remote we asked).
    pub peer: Peer,
    pub peers: Vec<SocketAddr>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSharingInitiator {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    /// Configured request size (from last [`PeerSharingMessage::Start`]).
    amount: u8,
    /// Delay between a reply and the next request.
    interval: Duration,
    /// Destination for every share result while this connection is live.
    reply_to: Option<StageRef<ShareResult>>,
    /// Outstanding timer for the next [`PeerSharingMessage::Tick`].
    timer: Option<ScheduleId>,
    /// True while waiting for `MsgSharePeers`.
    in_flight: bool,
}

impl PeerSharingInitiator {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, conn_id: ConnectionId) -> (State, Self) {
        (
            State::Idle,
            Self {
                muxer,
                peer,
                conn_id,
                amount: 0,
                interval: Duration::ZERO,
                reply_to: None,
                timer: None,
                in_flight: false,
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
            PeerSharingMessage::Start { amount, initial_delay, interval, reply_to } => {
                self.amount = amount;
                self.interval = interval;
                self.reply_to = Some(reply_to);
                // First request after initial_delay; connection tear-down cancels the timer with the stage.
                self.arm_timer(initial_delay, eff).await?;
                Ok((None, self))
            }
            PeerSharingMessage::Tick => {
                self.timer = None;
                if self.reply_to.is_none() {
                    return Ok((None, self));
                }
                match proto {
                    State::Idle if !self.in_flight => {
                        self.in_flight = true;
                        let amount = self.amount;
                        Ok((Some(InitiatorAction::ShareRequest { amount }), self))
                    }
                    State::Busy | State::Idle => {
                        // Still waiting for a reply (or already in flight); do not pipeline.
                        Ok((None, self))
                    }
                    State::Done => Ok((None, self)),
                }
            }
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
                InitiatorResult::SharePeers { peers } => {
                    if !self.in_flight {
                        warn!(protocols::peer_sharing::initiator::PROTOCOL_VIOLATION, reason = "no_request_in_flight");
                        return eff.terminate().await;
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
                    if let Some(reply_to) = self.reply_to.as_ref() {
                        eff.send(reply_to, ShareResult { peer: self.peer, peers }).await;
                    }
                    // Next request after the configured interval (same reply_to until stage ends).
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
        // Client agency in Idle: wait for local Start / Tick (no WantNext until we send).
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
