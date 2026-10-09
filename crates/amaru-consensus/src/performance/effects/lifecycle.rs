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

//! Handshake, disconnect, connect-failure, adversarial, and keep-alive observations.

#![expect(clippy::unit_arg)]

use std::time::Duration;

use amaru_kernel::Peer;
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue, require_perf};
use crate::{
    effect_timings,
    performance::{
        Performance,
        ops::{PeerOp, PerformanceOp},
    },
};

impl Performance {
    pub fn record_keepalive_rtt(peer: Peer, rtt: Duration, at: Instant) -> RecordKeepaliveRttEffect {
        RecordKeepaliveRttEffect { peer, rtt, at }
    }

    /// Record latest handshake peer-sharing willingness (`peer_sharing == 1` ⇒ advertisable).
    pub fn record_advertisability(peer: Peer, advertisable: bool, at: Instant) -> RecordAdvertisabilityEffect {
        RecordAdvertisabilityEffect { peer, advertisable, at }
    }

    /// Record a connection/protocol failure (increments `failure_count`).
    ///
    /// The worker bumps the peer-view generation.
    pub fn record_connection_failure(peer: Peer, at: Instant) -> RecordConnectionFailureEffect {
        RecordConnectionFailureEffect { peer, at }
    }

    pub fn clear_peer_availability(peer: Peer) -> ClearPeerAvailabilityEffect {
        ClearPeerAvailabilityEffect { peer }
    }

    /// Mark a peer as adversarial: clear claims/scores, keep a reputation stub with `adversarial = true`,
    /// and apply an adversarial malus impulse at `at`.
    pub fn peer_adversarial(peer: Peer, at: Instant) -> PeerAdversarialEffect {
        PeerAdversarialEffect { peer, at }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordKeepaliveRttEffect {
    pub(crate) peer: Peer,
    pub(crate) rtt: Duration,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordKeepaliveRttEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::RecordKeepaliveRtt { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordAdvertisabilityEffect {
    pub(crate) peer: Peer,
    pub(crate) advertisable: bool,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordAdvertisabilityEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__RecordAdvertisabilityEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::RecordAdvertisability { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordConnectionFailureEffect {
    pub(crate) peer: Peer,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordConnectionFailureEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__RecordConnectionFailureEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::RecordConnectionFailure { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClearPeerAvailabilityEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffectAPI for ClearPeerAvailabilityEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__ClearPeerAvailabilityEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::ClearPeerAvailability { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerAdversarialEffect {
    pub(crate) peer: Peer,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for PeerAdversarialEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::PeerAdversarial { effect: self.as_ref().clone() }));
        })
    }
}
