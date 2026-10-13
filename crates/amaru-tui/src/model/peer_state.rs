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

use std::{collections::VecDeque, time::Instant};

use amaru_observability::amaru::protocols;

use super::exponential_moving_average::ExponentialMovingAverage;
use crate::events::TelemetryRecord;

#[derive(Debug, Clone, Default, PartialEq)]
struct MeanMicros {
    average: ExponentialMovingAverage,
}

impl MeanMicros {
    fn record(&mut self, micros: u64, smoothing: usize) {
        self.average.record(micros as f64, smoothing);
    }

    fn mean(&self) -> Option<u64> {
        self.average.value().map(|micros| micros.round() as u64)
    }

    fn clear(&mut self) {
        self.average.clear();
    }
}

/// Temperature of one direction on a bearer, as reported by the peer-selection spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerUse {
    #[default]
    None,
    Maintenance,
    Diffusion,
}

impl PeerUse {
    pub fn from_label(label: &str) -> Self {
        match label {
            "maintenance" => Self::Maintenance,
            "diffusion" => Self::Diffusion,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeerState {
    pub address: String,
    /// Bootstrap name this address was resolved from, if that name was not already a socket address.
    pub candidate: Option<String>,
    pub inbound: bool,
    pub connected: bool,
    pub last_conn_id: Option<String>,
    pub last_rtt_micros: Option<u64>,
    pub last_reason: Option<String>,
    pub full_duplex: Option<bool>,
    /// Set once `local_use_applied` has been seen for this connection.
    local_use_seen: bool,
    /// Our use of this connection.
    pub local_use: PeerUse,
    /// The peer's use of us: we are its upstream.
    pub remote_use: PeerUse,
    slot_start_to_header: MeanMicros,
    query_header: MeanMicros,
    get_block: MeanMicros,
    adopt_block: MeanMicros,
    /// Live arrivals (slot onset to first body), newest at the back. Catch-up samples are not stored.
    live_arrivals: VecDeque<u64>,
    /// Sum of header-announcement points: rank 1 is 6, rank 2 is 3, rank 3 is 1.
    announcement_score: u64,
    /// Block height of the latest header this peer rolled forward.
    roll_forward_height: Option<u64>,
    /// Block height of the tip this peer advertises.
    peer_tip_height: Option<u64>,
    pub updated_at: Instant,
}

impl PeerState {
    pub fn new(address: String, updated_at: Instant) -> Self {
        Self {
            address,
            candidate: None,
            inbound: false,
            connected: false,
            last_conn_id: None,
            last_rtt_micros: None,
            last_reason: None,
            full_duplex: None,
            local_use_seen: false,
            local_use: PeerUse::None,
            remote_use: PeerUse::None,
            slot_start_to_header: MeanMicros::default(),
            query_header: MeanMicros::default(),
            get_block: MeanMicros::default(),
            adopt_block: MeanMicros::default(),
            live_arrivals: VecDeque::new(),
            announcement_score: 0,
            roll_forward_height: None,
            peer_tip_height: None,
            updated_at,
        }
    }

    pub fn mark_connected(&mut self, record: &TelemetryRecord) {
        let direction = protocols::peer_selection::peer::CONNECTED::direction(record);
        self.connected = true;
        self.inbound = direction == "Inbound";
        self.last_conn_id = record.conn_id();
        self.full_duplex = Some(protocols::peer_selection::peer::CONNECTED::full_duplex(record));
        // The span repeats the use already stored on the bearer. A `local_use_applied` seen
        // before adoption is the measurement and must not be replaced by this snapshot.
        if !self.local_use_seen {
            self.local_use = PeerUse::from_label(protocols::peer_selection::peer::CONNECTED::local_use(record));
        }
        self.last_reason = None;
        self.updated_at = record.at;
    }

    pub fn set_local_use(&mut self, label: &str, at: Instant) {
        self.local_use_seen = true;
        self.local_use = PeerUse::from_label(label);
        self.updated_at = at;
    }

    pub fn set_remote_use(&mut self, label: &str, at: Instant) {
        self.remote_use = PeerUse::from_label(label);
        self.updated_at = at;
    }

    pub fn update_rtt(&mut self, round_trip_micros: u64, at: Instant) {
        self.last_rtt_micros = Some(round_trip_micros);
        self.updated_at = at;
    }

    pub fn clear_slot_start_to_header(&mut self) {
        self.slot_start_to_header.clear();
    }

    pub fn is_stale(&self, now: Instant, inactivity_timeout: std::time::Duration) -> bool {
        now.saturating_duration_since(self.updated_at) > inactivity_timeout
    }

    pub fn record_header_lifecycle(
        &mut self,
        at: Instant,
        smoothing: usize,
        slot_start_to_header_micros: Option<u64>,
        query_header_micros: Option<u64>,
        get_block_micros: Option<u64>,
        adopt_block_micros: Option<u64>,
    ) {
        if let Some(micros) = slot_start_to_header_micros {
            self.slot_start_to_header.record(micros, smoothing);
        }
        if let Some(micros) = query_header_micros {
            self.query_header.record(micros, smoothing);
        }
        if let Some(micros) = get_block_micros {
            self.get_block.record(micros, smoothing);
        }
        if let Some(micros) = adopt_block_micros {
            self.adopt_block.record(micros, smoothing);
        }
        self.updated_at = at;
    }

    /// Add points for one logged header announcement. Rank 1 is 6, rank 2 is 3, rank 3 is 1.
    pub(crate) fn add_announcement_points(&mut self, points: u64, at: Instant) {
        self.announcement_score = self.announcement_score.saturating_add(points);
        self.updated_at = at;
    }

    pub fn announcement_score(&self) -> u64 {
        self.announcement_score
    }

    /// Record the header this peer just rolled forward and the tip it advertised with that header.
    ///
    /// The advertised tip can be one block behind that header: chainsync may send the next
    /// header before the peer counts it as its adopted tip. The header is still their chain.
    /// A later roll-forward replaces this tip, including when the advertisement is lower.
    /// A rollback or a new intersection is recorded separately and can also move it down.
    pub(crate) fn note_roll_forward(&mut self, height: u64, tip_height: u64, at: Instant) {
        self.roll_forward_height = Some(height);
        self.peer_tip_height = Some(tip_height.max(height));
        self.updated_at = at;
    }

    /// Record the tip this peer advertises, without a new roll-forward.
    ///
    /// A rollback can move the tip backward. A roll-forward header past that tip is pulled back with it.
    pub(crate) fn note_peer_tip(&mut self, tip_height: u64, at: Instant) {
        self.peer_tip_height = Some(tip_height);
        if self.roll_forward_height.is_some_and(|forward| forward > tip_height) {
            self.roll_forward_height = Some(tip_height);
        }
        self.updated_at = at;
    }

    pub fn roll_forward_height(&self) -> Option<u64> {
        self.roll_forward_height
    }

    pub fn peer_tip_height(&self) -> Option<u64> {
        self.peer_tip_height
    }

    /// Remember one live arrival latency. Only the last `capacity` samples are kept.
    pub(crate) fn record_live_arrival(&mut self, micros: u64, capacity: usize) {
        self.live_arrivals.push_back(micros);
        while self.live_arrivals.len() > capacity {
            self.live_arrivals.pop_front();
        }
    }

    /// Percent of retained live arrivals at or under `within_micros`, rounded to the nearest percent.
    /// `None` until a live arrival has been recorded.
    pub fn live_arrival_share_percent(&self, within_micros: u64) -> Option<u64> {
        let total = self.live_arrivals.len();
        if total == 0 {
            return None;
        }
        let within = self.live_arrivals.iter().filter(|micros| **micros <= within_micros).count();
        Some(((within as u64) * 100 + (total as u64) / 2) / (total as u64))
    }

    pub fn mean_query_header_micros(&self) -> Option<u64> {
        self.query_header.mean()
    }

    pub fn mean_slot_start_to_header_micros(&self) -> Option<u64> {
        self.slot_start_to_header.mean()
    }

    pub fn mean_get_block_micros(&self) -> Option<u64> {
        self.get_block.mean()
    }

    pub fn mean_adopt_block_micros(&self) -> Option<u64> {
        self.adopt_block.mean()
    }

    /// Name to show beside the socket address, if this peer came from a Host/SRV candidate.
    pub fn candidate_label(&self) -> Option<&str> {
        self.candidate.as_deref().filter(|candidate| *candidate != self.address)
    }
}
