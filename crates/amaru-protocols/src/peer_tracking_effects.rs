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

//! External effects for the peer-tracking resource.
//!
//! Stages pass the stage clock. The effect converts it with [`observed_at`] and calls
//! [`amaru_ouroboros::PeerTracking`]. The manager records a handshake, a close, and a failed
//! outbound attempt. The connection records applied local use. The manager reads
//! [`PeerTracking::may_initiate`](amaru_ouroboros::PeerTracking::may_initiate) when choosing
//! who to ask for blocks. The peer-sharing responder queries a reply and
//! records that it served the request. The initiator records addresses learned from a reply.
//! track_peers records an intersection miss. The keep-alive initiator records each matching
//! round trip. The peer-sharing responder records each inbound request it answers. The five
//! responders record which remote initiator protocols are open on the bearer.

use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

use amaru_kernel::Peer;
use amaru_observability::info;
use amaru_ouroboros::{
    CloseReason, ConnectionId, ConnectionRecord, MiniProtocolUse, PeerTrackingResource, RemoteProtocol,
};
use amaru_pure_stage::{BoxFuture, DeserializerGuards, Effects, ExternalEffectAPI, Instant, Resources, SendData};

use crate::peer_tracking::observed_at;

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<RecordConnectionEstablishedEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordConnectionClosedEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordConnectFailedEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordLocalUseAppliedEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordRemoteUseEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Option<MiniProtocolUse>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordKeepaliveRttEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordSharedPeersEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_ouroboros::SharedPeersRecorded>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordShareRequestServedEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecordUninterestingEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<QuerySharePeersEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Vec<SocketAddr>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<MayInitiateEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BTreeSet<ConnectionId>>().boxed(),
    ]
}

fn require_tracking(resources: &Resources) -> PeerTrackingResource {
    #[expect(clippy::expect_used)]
    resources.get::<PeerTrackingResource>().expect("peer tracking effect requires PeerTrackingResource").clone()
}

/// Effects facade over [`Effects::external`](amaru_pure_stage::Effects::external).
pub struct PeerTrack<'a, T>(&'a Effects<T>);

impl<'a, T> PeerTrack<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Self {
        Self(eff)
    }
}

impl<T> PeerTrack<'_, T> {
    pub fn record_connection_established(&self, conn: ConnectionRecord, at: Instant) -> BoxFuture<'static, ()> {
        self.0.external(RecordConnectionEstablishedEffect { conn, at })
    }

    pub fn record_connection_closed(
        &self,
        peer: Peer,
        conn_id: ConnectionId,
        reason: CloseReason,
        at: Instant,
    ) -> BoxFuture<'static, ()> {
        self.0.external(RecordConnectionClosedEffect { peer, conn_id, reason, at })
    }

    pub fn record_connect_failed(&self, peer: Peer, at: Instant) -> BoxFuture<'static, ()> {
        self.0.external(RecordConnectFailedEffect { peer, at })
    }

    pub fn record_local_use_applied(
        &self,
        peer: Peer,
        conn_id: ConnectionId,
        local_use: MiniProtocolUse,
        at: Instant,
    ) -> BoxFuture<'static, ()> {
        self.0.external(RecordLocalUseAppliedEffect { peer, conn_id, local_use, at })
    }

    fn record_remote_use(
        &self,
        peer: Peer,
        conn_id: ConnectionId,
        protocol: RemoteProtocol,
        active: bool,
        at: Instant,
    ) -> BoxFuture<'static, Option<MiniProtocolUse>> {
        self.0.external(RecordRemoteUseEffect { peer, conn_id, protocol, active, at })
    }

    /// Record one remote-initiator bit and emit `protocols::peer_selection::peer::REMOTE_USE`
    /// when the derived temperature changes.
    pub async fn note_remote_protocol(
        &self,
        peer: Peer,
        conn_id: ConnectionId,
        protocol: RemoteProtocol,
        active: bool,
    ) {
        let at = self.0.clock().await;
        let Some(remote_use) = self.record_remote_use(peer, conn_id, protocol, active, at).await else {
            return;
        };
        info!(
            protocols::peer_selection::peer::REMOTE_USE,
            peer,
            conn_id = conn_id.as_u64(),
            remote_use = remote_use.as_str(),
        );
    }

    pub fn record_keepalive_rtt(&self, peer: Peer, rtt: Duration, at: Instant) -> BoxFuture<'static, ()> {
        self.0.external(RecordKeepaliveRttEffect { peer, rtt, at })
    }

    pub fn record_shared_peers(
        &self,
        from: Peer,
        addrs: Vec<SocketAddr>,
        at: Instant,
    ) -> BoxFuture<'static, amaru_ouroboros::SharedPeersRecorded> {
        self.0.external(RecordSharedPeersEffect { from, addrs, at })
    }

    pub fn record_share_request_served(&self, requester: Peer, amount: u8, at: Instant) -> BoxFuture<'static, ()> {
        self.0.external(RecordShareRequestServedEffect { requester, amount, at })
    }

    pub fn record_uninteresting(
        &self,
        peer: Peer,
        conn_id: ConnectionId,
        after_rollback: bool,
        at: Instant,
    ) -> BoxFuture<'static, ()> {
        self.0.external(RecordUninterestingEffect { peer, conn_id, after_rollback, at })
    }

    pub fn query_share_peers(&self, requester: Peer, amount: u8, now: Instant) -> BoxFuture<'static, Vec<SocketAddr>> {
        self.0.external(QuerySharePeersEffect { requester, amount, now })
    }

    pub fn may_initiate(&self) -> BoxFuture<'static, BTreeSet<ConnectionId>> {
        self.0.external(MayInitiateEffect)
    }
}

/// Live bearers whose applied local use is diffusion.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MayInitiateEffect;

impl ExternalEffectAPI for MayInitiateEffect {
    type Response = BTreeSet<ConnectionId>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |_this| async move { tracking.may_initiate().await })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordConnectionEstablishedEffect {
    pub conn: ConnectionRecord,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordConnectionEstablishedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_connection_established(this.conn, observed_at(this.at)).await;
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordConnectionClosedEffect {
    pub peer: Peer,
    pub conn_id: ConnectionId,
    pub reason: CloseReason,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordConnectionClosedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_connection_closed(this.peer, this.conn_id, this.reason, observed_at(this.at));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordConnectFailedEffect {
    pub peer: Peer,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordConnectFailedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_connect_failed(this.peer, observed_at(this.at));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordLocalUseAppliedEffect {
    pub peer: Peer,
    pub conn_id: ConnectionId,
    pub local_use: MiniProtocolUse,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordLocalUseAppliedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_local_use_applied(this.peer, this.conn_id, this.local_use, observed_at(this.at));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordRemoteUseEffect {
    pub peer: Peer,
    pub conn_id: ConnectionId,
    pub protocol: RemoteProtocol,
    pub active: bool,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordRemoteUseEffect {
    type Response = Option<MiniProtocolUse>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_remote_use(this.peer, this.conn_id, this.protocol, this.active, observed_at(this.at)).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordKeepaliveRttEffect {
    pub peer: Peer,
    pub rtt: Duration,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordKeepaliveRttEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_keepalive_rtt(this.peer, this.rtt, observed_at(this.at));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordSharedPeersEffect {
    pub from: Peer,
    pub addrs: Vec<SocketAddr>,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordSharedPeersEffect {
    type Response = amaru_ouroboros::SharedPeersRecorded;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(
            move |this| async move { tracking.record_shared_peers(this.from, this.addrs, observed_at(this.at)).await },
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordShareRequestServedEffect {
    pub requester: Peer,
    pub amount: u8,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordShareRequestServedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_share_request_served(this.requester, this.amount, observed_at(this.at));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordUninterestingEffect {
    pub peer: Peer,
    pub conn_id: ConnectionId,
    pub after_rollback: bool,
    pub at: Instant,
}

impl ExternalEffectAPI for RecordUninterestingEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.record_uninteresting(this.peer, this.conn_id, this.after_rollback, observed_at(this.at));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QuerySharePeersEffect {
    pub requester: Peer,
    pub amount: u8,
    pub now: Instant,
}

impl ExternalEffectAPI for QuerySharePeersEffect {
    type Response = Vec<SocketAddr>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let tracking = require_tracking(&resources);
        self.wrap(move |this| async move {
            tracking.query_share_peers(this.requester, this.amount, observed_at(this.now)).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use amaru_kernel::Peer;
    use amaru_ouroboros::{
        CloseReason, ConnectionDirection, ConnectionId, ConnectionRecord, MiniProtocolUse, PeerTrackingResource,
        RemoteInitiators,
    };
    use amaru_pure_stage::{ExternalEffect, Instant, Resources};

    use super::*;
    use crate::peer_tracking::{InMemoryPeerTracking, observed_at};

    fn clock(elapsed_secs: u64, offset_secs: u64) -> Instant {
        Instant::at_offset(Duration::from_secs(elapsed_secs), Duration::from_secs(offset_secs))
    }

    #[test]
    fn observed_at_keeps_elapsed_and_offset_apart() {
        let instant = clock(11, 70_419_600);
        let observed = observed_at(instant);
        assert_eq!(observed.elapsed, Duration::from_secs(11));
        assert_eq!(observed.global_epoch_offset, Duration::from_secs(70_419_600));
    }

    #[test]
    fn effects_forward_each_argument_and_the_share_reply() {
        let recorder = Arc::new(InMemoryPeerTracking::new());
        let reply = SocketAddr::from(Peer::for_test(3202));
        recorder.set_share_reply(vec![reply]);
        let resources = Resources::default();
        resources.put::<PeerTrackingResource>(recorder.clone());

        let alice = Peer::for_test(3201);
        let at = clock(4, 8);
        let seen = observed_at(at);
        let conn = ConnectionRecord {
            peer: alice,
            conn_id: ConnectionId::initial(),
            direction: ConnectionDirection::Inbound,
            full_duplex: true,
            advertisable: true,
            local_use: MiniProtocolUse::Maintenance,
            remote_initiators: RemoteInitiators::default(),
            established_at: seen,
        };
        let addr = SocketAddr::from(Peer::for_test(3203));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let drive = |effect: Box<dyn ExternalEffect>| rt.block_on(effect.run(resources.clone()));

        drive(Box::new(RecordConnectionEstablishedEffect { conn: conn.clone(), at }));
        drive(Box::new(RecordConnectionClosedEffect {
            peer: alice,
            conn_id: conn.conn_id,
            reason: CloseReason::BearerEnded,
            at,
        }));
        drive(Box::new(RecordConnectFailedEffect { peer: alice, at }));
        drive(Box::new(RecordLocalUseAppliedEffect {
            peer: alice,
            conn_id: conn.conn_id,
            local_use: MiniProtocolUse::Diffusion,
            at,
        }));
        drive(Box::new(RecordKeepaliveRttEffect { peer: alice, rtt: Duration::from_millis(15), at }));
        drive(Box::new(RecordSharedPeersEffect { from: alice, addrs: vec![addr], at }));
        drive(Box::new(RecordShareRequestServedEffect { requester: alice, amount: 4, at }));
        drive(Box::new(RecordUninterestingEffect { peer: alice, conn_id: conn.conn_id, after_rollback: false, at }));
        let response = rt.block_on(
            (Box::new(QuerySharePeersEffect { requester: alice, amount: 4, now: at }) as Box<dyn ExternalEffect>)
                .run(resources.clone()),
        );
        let addrs = *response.cast::<Vec<SocketAddr>>().expect("share reply");

        assert_eq!(recorder.established(), vec![(conn.clone(), seen)]);
        assert_eq!(recorder.closed(), vec![(alice, conn.conn_id, CloseReason::BearerEnded, seen)]);
        assert_eq!(recorder.connect_failures(), vec![(alice, seen)]);
        assert_eq!(recorder.local_uses(), vec![(alice, conn.conn_id, MiniProtocolUse::Diffusion, seen)]);
        assert_eq!(recorder.keepalives(), vec![(alice, Duration::from_millis(15), seen)]);
        assert_eq!(recorder.shared_peers(), vec![(alice, vec![addr], seen)]);
        assert_eq!(recorder.share_requests(), vec![(alice, 4, seen)]);
        assert_eq!(recorder.uninteresting(), vec![(alice, conn.conn_id, false, seen)]);
        assert_eq!(recorder.share_queries(), vec![(alice, 4, seen)]);
        assert_eq!(addrs, vec![reply]);
    }
}
