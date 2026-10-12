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

//! Reputation queries the peer-sharing filters use.

use amaru_kernel::Peer;
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue_query, require_perf};
use crate::performance::{
    PeerShareFlags, PeerSnapshot, Performance,
    ops::{PeerOp, PerformanceOp},
};

impl Performance {
    pub fn share_flags(peer: Peer) -> ShareFlagsEffect {
        ShareFlagsEffect { peer }
    }

    pub fn snapshot(peer: Peer) -> SnapshotEffect {
        SnapshotEffect { peer }
    }

    pub fn ok_for_sharing(peer: Peer, now: Instant) -> OkForSharingEffect {
        OkForSharingEffect { peer, now }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShareFlagsEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffectAPI for ShareFlagsEffect {
    type Response = Option<PeerShareFlags>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::ShareFlags { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffectAPI for SnapshotEffect {
    type Response = Option<PeerSnapshot>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::Snapshot { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OkForSharingEffect {
    pub(crate) peer: Peer,
    pub(crate) now: Instant,
}

impl ExternalEffectAPI for OkForSharingEffect {
    type Response = bool;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::OkForSharing { effect: this, reply })).await
        })
    }
}
