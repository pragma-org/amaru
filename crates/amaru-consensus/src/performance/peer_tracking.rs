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

//! [`Performance`] as the peer-tracking resource.
//!
//! Each method enqueues one operation on the same worker and the same FIFO as header and pace
//! updates. A query enqueued after a write observes that write. A share reply copies candidate
//! fields on that worker; the sample is drawn here, after the copy returns.

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use amaru_kernel::Peer;
use amaru_ouroboros::{
    CloseReason, ConnectionId, ConnectionRecord, MiniProtocolUse, ObservedAt, PeerTracking, PeerTrackingFuture,
    PeerTrackingResource, RemoteProtocol,
};
use amaru_pure_stage::Resources;
use tokio::sync::oneshot;

use super::{Performance, ResourcePerformance, ops::PerformanceOp};
use crate::performance::{
    ops::PeerOp,
    peers::{instant_of, sample_share_peers, share_reply_seed},
};

impl Performance {
    /// Install this worker under both resource names. They share one allocation and one queue.
    ///
    /// The returned callback joins the worker. Drop every [`Performance`] handle first.
    pub fn install(self, resources: &Resources) -> impl FnOnce() -> std::thread::Result<()> + Send + Sync + 'static {
        let performance = Arc::new(self);
        let join = performance.shutdown_callback();
        resources.put::<ResourcePerformance>(Arc::clone(&performance));
        resources.put::<PeerTrackingResource>(performance);
        join
    }
}

impl PeerTracking for Performance {
    fn record_connection_established(&self, conn: ConnectionRecord, at: ObservedAt) -> PeerTrackingFuture<()> {
        let this = self.clone();
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            this.submit(PerformanceOp::Peer(PeerOp::RecordConnectionEstablished { conn, at, reply }));
            #[expect(clippy::expect_used)]
            rx.await.expect("performance worker dropped connection-established record")
        })
    }

    fn record_connection_closed(&self, peer: Peer, conn_id: ConnectionId, reason: CloseReason, at: ObservedAt) {
        self.submit(PerformanceOp::Peer(PeerOp::RecordConnectionClosed { peer, conn_id, reason, at }));
    }

    fn record_connect_failed(&self, peer: Peer, at: ObservedAt) {
        self.submit(PerformanceOp::Peer(PeerOp::RecordConnectFailed { peer, at }));
    }

    fn record_local_use_applied(&self, peer: Peer, conn_id: ConnectionId, local_use: MiniProtocolUse, at: ObservedAt) {
        self.submit(PerformanceOp::Peer(PeerOp::RecordLocalUseApplied { peer, conn_id, local_use, at }));
    }

    fn may_initiate(&self) -> PeerTrackingFuture<BTreeSet<ConnectionId>> {
        let this = self.clone();
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            this.submit(PerformanceOp::Peer(PeerOp::MayInitiate { reply }));
            #[expect(clippy::expect_used)]
            rx.await.expect("performance worker dropped may-initiate query")
        })
    }

    fn record_remote_use(
        &self,
        peer: Peer,
        conn_id: ConnectionId,
        protocol: RemoteProtocol,
        active: bool,
        at: ObservedAt,
    ) -> PeerTrackingFuture<Option<MiniProtocolUse>> {
        let this = self.clone();
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            this.submit(PerformanceOp::Peer(PeerOp::RecordRemoteUse { peer, conn_id, protocol, active, at, reply }));
            #[expect(clippy::expect_used)]
            rx.await.expect("performance worker dropped remote-use record")
        })
    }

    fn record_keepalive_rtt(&self, peer: Peer, rtt: Duration, at: ObservedAt) {
        self.submit(PerformanceOp::Peer(PeerOp::RecordKeepaliveSample { peer, rtt, at }));
    }

    fn record_shared_peers(
        &self,
        from: Peer,
        addrs: Vec<SocketAddr>,
        at: ObservedAt,
    ) -> PeerTrackingFuture<amaru_ouroboros::SharedPeersRecorded> {
        let this = self.clone();
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            this.submit(PerformanceOp::Peer(PeerOp::RecordSharedPeers { from, addrs, at, reply }));
            #[expect(clippy::expect_used)]
            let result = rx.await.expect("performance worker dropped shared-peer ingest");
            amaru_ouroboros::SharedPeersRecorded { added: result.added, total: result.total }
        })
    }

    fn record_share_request_served(&self, requester: Peer, amount: u8, at: ObservedAt) {
        self.submit(PerformanceOp::Peer(PeerOp::RecordShareRequestServed { requester, amount, at }));
    }

    fn record_uninteresting(&self, peer: Peer, conn_id: ConnectionId, after_rollback: bool, at: ObservedAt) {
        self.submit(PerformanceOp::Peer(PeerOp::RecordUninteresting { peer, conn_id, after_rollback, at }));
    }

    fn query_share_peers(&self, requester: Peer, amount: u8, now: ObservedAt) -> PeerTrackingFuture<Vec<SocketAddr>> {
        let this = self.clone();
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            let now = instant_of(now);
            this.submit(PerformanceOp::Peer(PeerOp::ShareReplyCandidates { now, reply }));
            #[expect(clippy::expect_used)]
            let candidates = rx.await.expect("performance worker dropped share-peer reply");
            sample_share_peers(&requester, amount, &candidates, share_reply_seed(&requester))
        })
    }
}
