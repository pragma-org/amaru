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

//! Live bearers and the other population facts the peer-tracking methods record.
//!
//! Claim clearing on the last close uses the existing availability clear: scores (including the
//! keep-alive summary), reputation, and the share-request row stay; tips go. An adversarial mark
//! clears scores and that share-request row.

use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

use amaru_kernel::Peer;
use amaru_ouroboros::{CloseReason, ConnectionId, ConnectionRecord, MiniProtocolUse, ObservedAt, RemoteProtocol};
use amaru_pure_stage::Instant;

use super::{PEER_RECORD_CAP, PeerPerformance};

#[derive(Debug)]
pub(super) struct LiveConnection {
    pub(super) record: ConnectionRecord,
    pub(super) use_applied_at: Option<ObservedAt>,
}

#[derive(Debug)]
pub(super) struct CloseRecord {
    pub(super) conn_id: ConnectionId,
    pub(super) reason: CloseReason,
    pub(super) at: ObservedAt,
}

/// Width of one share-request rate window. Two windows are kept: the current one and the previous.
pub const SHARE_REQUEST_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(super) struct ShareRequests {
    pub(super) count: u32,
    pub(super) last_amount: u8,
    pub(super) last_at: ObservedAt,
    pub(super) window_start: ObservedAt,
    pub(super) current_window: u32,
    pub(super) previous_window: u32,
}

/// Copy of one peer's served share requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareRequestRecord {
    pub count: u32,
    pub last_amount: u8,
    pub last_at: ObservedAt,
    pub window_start: ObservedAt,
    pub current_window: u32,
    pub previous_window: u32,
}

/// Latest intersection-not-found mark for one peer.
#[derive(Debug)]
pub(super) struct UninterestingRecord {
    pub(super) conn_id: ConnectionId,
    pub(super) after_rollback: bool,
    pub(super) generation: u64,
}

pub(crate) fn instant_of(at: ObservedAt) -> Instant {
    Instant::at_offset(at.elapsed, at.global_epoch_offset)
}

fn roll_share_window(entry: &mut ShareRequests, at: ObservedAt) {
    let elapsed = at.elapsed.saturating_sub(entry.window_start.elapsed);
    if elapsed < SHARE_REQUEST_WINDOW {
        entry.current_window = entry.current_window.saturating_add(1);
        return;
    }
    let windows = u32::try_from(elapsed.as_nanos() / SHARE_REQUEST_WINDOW.as_nanos()).unwrap_or(u32::MAX);
    entry.previous_window = if windows == 1 { entry.current_window } else { 0 };
    entry.current_window = 1;
    let advance = SHARE_REQUEST_WINDOW.saturating_mul(windows);
    entry.window_start =
        ObservedAt::new(entry.window_start.elapsed.saturating_add(advance), entry.window_start.global_epoch_offset);
}

impl PeerPerformance {
    pub fn record_connection_established(&mut self, conn: ConnectionRecord, at: ObservedAt) {
        let peer = conn.peer;
        let advertisable = conn.advertisable;
        self.connections.insert(conn.conn_id, LiveConnection { record: conn, use_applied_at: None });
        self.record_advertisability(peer, advertisable, instant_of(at));
        self.note_activity(peer, instant_of(at));
        self.bump_generation();
    }

    /// Forget a bearer this map recorded for `peer`.
    ///
    /// A close for an unknown id, or for a different peer than the one stored under that id,
    /// changes nothing. The last remaining bearer clears claims.
    pub fn record_connection_closed(&mut self, peer: Peer, conn_id: ConnectionId, reason: CloseReason, at: ObservedAt) {
        let Some(live) = self.connections.get(&conn_id) else {
            return;
        };
        if live.record.peer != peer {
            return;
        }
        self.connections.remove(&conn_id);
        self.last_close.insert(peer, CloseRecord { conn_id, reason, at });
        self.note_activity(peer, instant_of(at));
        let still_live = self.connections.values().any(|live| live.record.peer == peer);
        if !still_live {
            self.clear_availability(&peer);
        }
        self.drop_uninteresting_if_bearer_gone(peer);
        self.bump_generation();
    }

    pub fn record_connect_failed(&mut self, peer: Peer, at: ObservedAt) {
        self.last_connect_failure.insert(peer, at);
        // `record_connection_failure` bumps the generation.
        self.record_connection_failure(peer, instant_of(at));
    }

    pub fn record_local_use_applied(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        local_use: MiniProtocolUse,
        at: ObservedAt,
    ) {
        let Some(live) = self.connections.get_mut(&conn_id) else {
            return;
        };
        if live.record.peer != peer {
            return;
        }
        live.record.local_use = local_use;
        live.use_applied_at = Some(at);
        self.note_activity(peer, instant_of(at));
        self.bump_generation();
    }

    /// Bearers the manager may ask for blocks: applied local use is diffusion.
    pub fn may_initiate(&self) -> BTreeSet<ConnectionId> {
        self.connections
            .iter()
            .filter(|(_, live)| live.record.local_use == MiniProtocolUse::Diffusion)
            .map(|(conn_id, _)| *conn_id)
            .collect()
    }

    /// Returns the new derived temperature when it changed.
    ///
    /// A bit flip that leaves the temperature in place refreshes activity and does not move
    /// the generation.
    pub fn record_remote_use(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        protocol: RemoteProtocol,
        active: bool,
        at: ObservedAt,
    ) -> Option<MiniProtocolUse> {
        let live = self.connections.get_mut(&conn_id)?;
        if live.record.peer != peer {
            return None;
        }
        let prev = live.record.remote_initiators.derived();
        if !live.record.remote_initiators.set(protocol, active) {
            return None;
        }
        let next = live.record.remote_initiators.derived();
        self.note_activity(peer, instant_of(at));
        if prev != next {
            self.bump_generation();
        }
        (prev != next).then_some(next)
    }

    pub fn record_keepalive_sample(&mut self, peer: Peer, rtt: Duration, at: ObservedAt) {
        self.record_keepalive_rtt(peer, rtt, instant_of(at));
    }

    pub fn record_shared_peers(
        &mut self,
        from: &Peer,
        addrs: &[SocketAddr],
        at: ObservedAt,
    ) -> super::SharedIngestResult {
        self.last_shared_at.insert(*from, at);
        self.note_activity(*from, instant_of(at));
        self.ingest_shared_peers(from, addrs, instant_of(at))
    }

    /// Keep one mark per peer, at the generation this write just advanced.
    ///
    /// A mark whose bearer is already gone is not stored and does not move the generation.
    pub fn record_uninteresting(&mut self, peer: Peer, conn_id: ConnectionId, after_rollback: bool, at: ObservedAt) {
        let live = self.connections.values().any(|live| live.record.peer == peer && live.record.conn_id == conn_id);
        if !live {
            return;
        }
        self.bump_generation();
        self.uninteresting.insert(peer, UninterestingRecord { conn_id, after_rollback, generation: self.generation });
        self.note_activity(peer, instant_of(at));
    }

    fn drop_uninteresting_if_bearer_gone(&mut self, peer: Peer) {
        let Some(mark) = self.uninteresting.get(&peer) else {
            return;
        };
        let live =
            self.connections.values().any(|live| live.record.peer == peer && live.record.conn_id == mark.conn_id);
        if !live {
            self.uninteresting.remove(&peer);
        }
    }

    pub fn record_share_request_served(&mut self, requester: Peer, amount: u8, at: ObservedAt) {
        if !self.activity.contains_key(&requester) && self.activity.len() >= PEER_RECORD_CAP {
            return;
        }
        self.note_activity(requester, instant_of(at));
        let entry = self.share_requests.entry(requester).or_insert(ShareRequests {
            count: 0,
            last_amount: amount,
            last_at: at,
            window_start: at,
            current_window: 0,
            previous_window: 0,
        });
        entry.count = entry.count.saturating_add(1);
        entry.last_amount = amount;
        entry.last_at = at;
        roll_share_window(entry, at);
    }

    pub fn connection(&self, conn_id: ConnectionId) -> Option<&ConnectionRecord> {
        self.connections.get(&conn_id).map(|live| &live.record)
    }

    pub fn use_applied_at(&self, conn_id: ConnectionId) -> Option<ObservedAt> {
        self.connections.get(&conn_id).and_then(|live| live.use_applied_at)
    }

    pub fn last_close(&self, peer: &Peer) -> Option<(ConnectionId, CloseReason, ObservedAt)> {
        self.last_close.get(peer).map(|close| (close.conn_id, close.reason, close.at))
    }

    pub fn last_connect_failure(&self, peer: &Peer) -> Option<ObservedAt> {
        self.last_connect_failure.get(peer).copied()
    }

    pub fn last_shared_at(&self, peer: &Peer) -> Option<ObservedAt> {
        self.last_shared_at.get(peer).copied()
    }

    pub fn share_requests(&self, peer: &Peer) -> Option<ShareRequestRecord> {
        self.share_requests.get(peer).map(|row| ShareRequestRecord {
            count: row.count,
            last_amount: row.last_amount,
            last_at: row.last_at,
            window_start: row.window_start,
            current_window: row.current_window,
            previous_window: row.previous_window,
        })
    }

    pub fn query_share_peers(&self, requester: &Peer, amount: u8, now: ObservedAt) -> Vec<SocketAddr> {
        self.select_share_peers(requester, amount, instant_of(now))
    }
}
