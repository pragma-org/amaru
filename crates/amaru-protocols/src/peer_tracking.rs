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

//! In-memory [`PeerTracking`] for protocol tests.
//!
//! Protocols cannot depend on the consensus worker. This recorder stores each call so a test
//! can assert what an effect wrote, and it returns a scripted share reply.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use amaru_kernel::Peer;
use amaru_ouroboros::{
    CloseReason, ConnectionId, ConnectionRecord, LocalUse, ObservedAt, PeerTracking, PeerTrackingFuture,
};
use amaru_pure_stage::Instant;

/// Split a stage-clock reading into the two durations [`ObservedAt`] stores.
pub fn observed_at(instant: Instant) -> ObservedAt {
    let elapsed = instant.sim_elapsed();
    let global_epoch_offset = instant.duration_since_global_epoch().saturating_sub(elapsed);
    ObservedAt::new(elapsed, global_epoch_offset)
}

#[derive(Debug, Default)]
struct Log {
    established: Vec<(ConnectionRecord, ObservedAt)>,
    closed: Vec<(Peer, ConnectionId, CloseReason, ObservedAt)>,
    connect_failures: Vec<(Peer, ObservedAt)>,
    local_uses: Vec<(Peer, ConnectionId, LocalUse, ObservedAt)>,
    keepalives: Vec<(Peer, Duration, ObservedAt)>,
    shared: Vec<(Peer, Vec<SocketAddr>, ObservedAt)>,
    share_requests: Vec<(Peer, u8, ObservedAt)>,
    uninteresting: Vec<(Peer, ConnectionId, bool, ObservedAt)>,
    queries: Vec<(Peer, u8, ObservedAt)>,
    share_reply: Vec<SocketAddr>,
}

/// Call recorder. Share replies are whatever [`Self::set_share_reply`] last set (empty by default).
#[derive(Debug, Default)]
pub struct InMemoryPeerTracking {
    inner: Mutex<Log>,
}

impl InMemoryPeerTracking {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_share_reply(&self, addrs: Vec<SocketAddr>) {
        self.lock().share_reply = addrs;
    }

    pub fn established(&self) -> Vec<(ConnectionRecord, ObservedAt)> {
        self.lock().established.clone()
    }

    pub fn closed(&self) -> Vec<(Peer, ConnectionId, CloseReason, ObservedAt)> {
        self.lock().closed.clone()
    }

    pub fn connect_failures(&self) -> Vec<(Peer, ObservedAt)> {
        self.lock().connect_failures.clone()
    }

    pub fn local_uses(&self) -> Vec<(Peer, ConnectionId, LocalUse, ObservedAt)> {
        self.lock().local_uses.clone()
    }

    pub fn keepalives(&self) -> Vec<(Peer, Duration, ObservedAt)> {
        self.lock().keepalives.clone()
    }

    pub fn shared_peers(&self) -> Vec<(Peer, Vec<SocketAddr>, ObservedAt)> {
        self.lock().shared.clone()
    }

    pub fn share_requests(&self) -> Vec<(Peer, u8, ObservedAt)> {
        self.lock().share_requests.clone()
    }

    pub fn uninteresting(&self) -> Vec<(Peer, ConnectionId, bool, ObservedAt)> {
        self.lock().uninteresting.clone()
    }

    pub fn share_queries(&self) -> Vec<(Peer, u8, ObservedAt)> {
        self.lock().queries.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Log> {
        #[expect(clippy::expect_used)]
        self.inner.lock().expect("peer tracking recorder lock poisoned")
    }
}

impl PeerTracking for InMemoryPeerTracking {
    fn record_connection_established(&self, conn: ConnectionRecord, at: ObservedAt) {
        self.lock().established.push((conn, at));
    }

    fn record_connection_closed(&self, peer: Peer, conn_id: ConnectionId, reason: CloseReason, at: ObservedAt) {
        self.lock().closed.push((peer, conn_id, reason, at));
    }

    fn record_connect_failed(&self, peer: Peer, at: ObservedAt) {
        self.lock().connect_failures.push((peer, at));
    }

    fn record_local_use_applied(&self, peer: Peer, conn_id: ConnectionId, local_use: LocalUse, at: ObservedAt) {
        self.lock().local_uses.push((peer, conn_id, local_use, at));
    }

    fn may_initiate(&self) -> PeerTrackingFuture<BTreeSet<ConnectionId>> {
        let log = self.lock();
        let mut use_of: BTreeMap<ConnectionId, LocalUse> = BTreeMap::new();
        for (conn, _) in &log.established {
            use_of.insert(conn.conn_id, conn.local_use);
        }
        for (_, conn_id, local_use, _) in &log.local_uses {
            use_of.insert(*conn_id, *local_use);
        }
        for (_, conn_id, _, _) in &log.closed {
            use_of.remove(conn_id);
        }
        let ids =
            use_of.into_iter().filter(|(_, local_use)| *local_use == LocalUse::Diffusion).map(|(id, _)| id).collect();
        Box::pin(async move { ids })
    }

    fn record_keepalive_rtt(&self, peer: Peer, rtt: Duration, at: ObservedAt) {
        self.lock().keepalives.push((peer, rtt, at));
    }

    fn record_shared_peers(
        &self,
        from: Peer,
        addrs: Vec<SocketAddr>,
        at: ObservedAt,
    ) -> PeerTrackingFuture<amaru_ouroboros::SharedPeersRecorded> {
        let added = addrs.len();
        self.lock().shared.push((from, addrs, at));
        Box::pin(async move { amaru_ouroboros::SharedPeersRecorded { added, total: added } })
    }

    fn record_share_request_served(&self, requester: Peer, amount: u8, at: ObservedAt) {
        self.lock().share_requests.push((requester, amount, at));
    }

    fn record_uninteresting(&self, peer: Peer, conn_id: ConnectionId, after_rollback: bool, at: ObservedAt) {
        self.lock().uninteresting.push((peer, conn_id, after_rollback, at));
    }

    fn query_share_peers(&self, requester: Peer, amount: u8, now: ObservedAt) -> PeerTrackingFuture<Vec<SocketAddr>> {
        let reply = {
            let mut log = self.lock();
            log.queries.push((requester, amount, now));
            log.share_reply.clone()
        };
        Box::pin(async move { reply })
    }
}
