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

//! Chain-tip observations: intersection, announcement, delivery, rollback.

#![expect(clippy::unit_arg)]

use std::time::Duration;

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point};
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue, enqueue_and_emit_telemetry, require_perf};
use crate::performance::{
    Performance,
    ops::{PeerOp, PerformanceOp},
};

impl Performance {
    pub fn record_intersection(
        peer: Peer,
        current: Point,
        parent: Option<HeaderHash>,
        at: Instant,
    ) -> RecordIntersectionEffect {
        RecordIntersectionEffect { peer, current, parent, at }
    }

    /// Record that `peer` announced `header`.
    ///
    /// `already_stored` is set when the chain store already holds the header. That announcement
    /// can extend an open list (ranks 2 and 3) but does not start a new rank-1 line or lifecycle.
    pub fn record_header_announcement(
        peer: Peer,
        header: Point,
        parent: Option<HeaderHash>,
        at: Instant,
        slot_start_to_header_micros: u64,
        slot_onset: Duration,
        already_stored: bool,
    ) -> RecordHeaderAnnouncementEffect {
        RecordHeaderAnnouncementEffect {
            peer,
            header,
            parent,
            at,
            slot_start_to_header_micros,
            slot_onset,
            already_stored,
        }
    }

    pub fn record_block_delivery(
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        parent: Option<HeaderHash>,
        at: Instant,
        response: Duration,
        bytes: u64,
    ) -> RecordBlockDeliveryEffect {
        RecordBlockDeliveryEffect { peer, hash, height, parent, at, response, bytes }
    }

    pub fn record_rollback(peer: Peer, point: Point, parent: Option<HeaderHash>, at: Instant) -> RecordRollbackEffect {
        RecordRollbackEffect { peer, point, parent, at }
    }
}

// ---------------------------------------------------------------------------
// Effect types
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordIntersectionEffect {
    pub(crate) peer: Peer,
    pub(crate) current: Point,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordIntersectionEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::RecordIntersection { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderAnnouncementEffect {
    pub(crate) peer: Peer,
    pub(crate) header: Point,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
    /// Stage-computed interval from virtual slot start to header reception.
    pub(crate) slot_start_to_header_micros: u64,
    /// Slot onset as a duration since the global epoch.
    pub(crate) slot_onset: Duration,
    /// The chain store already held this header. Used to avoid a fresh rank-1 announcement.
    pub(crate) already_stored: bool,
}

impl ExternalEffectAPI for RecordHeaderAnnouncementEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Peer(PeerOp::RecordHeaderAnnouncement { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockDeliveryEffect {
    pub(crate) peer: Peer,
    pub(crate) hash: HeaderHash,
    pub(crate) height: BlockHeight,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
    pub(crate) response: Duration,
    pub(crate) bytes: u64,
}

impl ExternalEffectAPI for RecordBlockDeliveryEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Peer(PeerOp::RecordBlockDelivery { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordRollbackEffect {
    pub(crate) peer: Peer,
    pub(crate) point: Point,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordRollbackEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::RecordRollback { effect: self.as_ref().clone() }));
        })
    }
}
