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

//! Peer-population observations shared by the network stack and consensus.
//!
//! A message is for work the receiver must act on immediately (which peer to ask next, which
//! chain to adopt, an adversarial disconnect). Population bookkeeping — who is connected, how
//! the bearer is used, sharing results, round-trip samples — is written here and read later.
//!
//! Callers pass the stage clock as [`ObservedAt`]. The resource does not read a wall clock.

use std::{collections::BTreeSet, future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

use amaru_kernel::Peer;

use crate::{ConnectionDirection, ConnectionId};

/// Stage-clock reading: simulation elapsed time and the global epoch offset, stored separately.
///
/// This is the same pair the stage clock serializes. Splitting it here keeps this crate free of
/// the simulator clock type, and lets a worker rebuild that clock without folding the offset
/// into the elapsed time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ObservedAt {
    pub elapsed: Duration,
    pub global_epoch_offset: Duration,
}

impl ObservedAt {
    pub const fn new(elapsed: Duration, global_epoch_offset: Duration) -> Self {
        Self { elapsed, global_epoch_offset }
    }
}

/// Which initiator groups this node intends to run on a bearer.
///
/// Ordered by inclusion: `Maintenance` includes `None`, and `Diffusion` includes maintenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum LocalUse {
    None,
    Maintenance,
    Diffusion,
}

impl LocalUse {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Maintenance => "maintenance",
            Self::Diffusion => "diffusion",
        }
    }
}

/// Why a live bearer ended.
///
/// A failed dial is [`PeerTracking::record_connect_failed`], not a close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum CloseReason {
    /// This node decided to drop the bearer.
    LocalDisconnect,
    /// The bearer ended without a local disconnect (remote close or the connection task died).
    BearerEnded,
}

/// One established bearer.
///
/// `established_at` is when the handshake completed. Later observations pass their own time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionRecord {
    pub peer: Peer,
    pub conn_id: ConnectionId,
    pub direction: ConnectionDirection,
    pub full_duplex_capable: bool,
    pub full_duplex: bool,
    pub advertisable: bool,
    pub local_use: LocalUse,
    pub established_at: ObservedAt,
}

/// How many shared addresses were new, and the pool size after the ingest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharedPeersRecorded {
    pub added: usize,
    pub total: usize,
}

/// Future returned by [`PeerTracking`] methods that wait for the worker.
pub type PeerTrackingFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Peer-population resource.
///
/// Each method is one in-memory update, or a copy of a small result, on the single worker that
/// owns the performance maps. A share reply copies the candidate fields; the caller samples.
/// Ranking for peer selection is not done here.
pub trait PeerTracking: Send + Sync + 'static {
    /// Handshake succeeded. Also marks the peer ever-connected and records `advertisable`.
    fn record_connection_established(&self, conn: ConnectionRecord, at: ObservedAt);

    /// A bearer that had completed handshake is gone.
    ///
    /// When it was the peer's last live bearer, claims for that peer are cleared. Reputation
    /// (ever-connected, malus, adversarial) is kept.
    fn record_connection_closed(&self, peer: Peer, conn_id: ConnectionId, reason: CloseReason, at: ObservedAt);

    /// Outbound dial failed before handshake. Does not mark the peer ever-connected.
    fn record_connect_failed(&self, peer: Peer, at: ObservedAt);

    /// The connection task applied `local_use` on an established bearer.
    fn record_local_use_applied(&self, peer: Peer, conn_id: ConnectionId, local_use: LocalUse, at: ObservedAt);

    /// Live bearers whose applied local use is [`LocalUse::Diffusion`].
    ///
    /// The manager uses this instead of a message from the connection when choosing who may be
    /// asked for blocks. A query enqueued after [`Self::record_local_use_applied`] sees that write.
    fn may_initiate(&self) -> PeerTrackingFuture<BTreeSet<ConnectionId>>;

    /// One keep-alive round trip.
    fn record_keepalive_rtt(&self, peer: Peer, rtt: Duration, at: ObservedAt);

    /// Addresses learned from `from`.
    ///
    /// The worker applies the ingest and returns how many addresses were new and how large the
    /// shared pool is afterwards. The caller logs that result.
    fn record_shared_peers(
        &self,
        from: Peer,
        addrs: Vec<SocketAddr>,
        at: ObservedAt,
    ) -> PeerTrackingFuture<SharedPeersRecorded>;

    /// An inbound share request from `requester` was answered.
    fn record_share_request_served(&self, requester: Peer, amount: u8, at: ObservedAt);

    /// ChainSync found no usable intersection on this bearer.
    ///
    /// The latest mark for `peer` replaces any earlier one. The write bumps the generation peer
    /// selection watches. It does not choose the next header or block.
    fn record_uninteresting(&self, peer: Peer, conn_id: ConnectionId, after_rollback: bool, at: ObservedAt);

    /// Addresses to send in a share reply.
    ///
    /// The worker copies the candidates. The caller draws the sample from the requester seed.
    fn query_share_peers(&self, requester: Peer, amount: u8, now: ObservedAt) -> PeerTrackingFuture<Vec<SocketAddr>>;
}

/// Resource type installed beside the consensus performance handle. Both names refer to one worker.
pub type PeerTrackingResource = Arc<dyn PeerTracking>;
