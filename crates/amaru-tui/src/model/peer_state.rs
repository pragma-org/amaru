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

use std::time::Instant;

use amaru_observability::amaru::protocols;

use crate::events::TelemetryRecord;

#[derive(Debug, Clone, PartialEq)]
pub struct PeerState {
    pub address: String,
    pub inbound: bool,
    pub outbound: bool,
    pub connected: bool,
    pub trusted: bool,
    pub last_conn_id: Option<String>,
    pub last_rtt_micros: Option<u64>,
    pub last_reason: Option<String>,
    pub full_duplex: Option<bool>,
    pub full_duplex_capable: Option<bool>,
    pub updated_at: Instant,
}

impl PeerState {
    pub fn new(address: String, trusted: bool, updated_at: Instant) -> Self {
        Self {
            address,
            inbound: false,
            outbound: false,
            connected: false,
            trusted,
            last_conn_id: None,
            last_rtt_micros: None,
            last_reason: None,
            full_duplex: None,
            full_duplex_capable: None,
            updated_at,
        }
    }

    pub fn mark_connected(&mut self, record: &TelemetryRecord) {
        let direction = protocols::peer_selection::peer::CONNECTED::direction(record);
        self.connected = true;
        self.inbound |= direction == "Inbound";
        self.outbound |= direction == "Outbound";
        self.last_conn_id = record.conn_id();
        self.full_duplex = Some(protocols::peer_selection::peer::CONNECTED::full_duplex(record));
        self.full_duplex_capable = Some(protocols::peer_selection::peer::CONNECTED::full_duplex_capable(record));
        self.last_reason = None;
        self.updated_at = record.at;
    }

    pub fn mark_disconnected(&mut self, record: &TelemetryRecord) {
        self.connected = false;
        self.last_reason = protocols::peer_selection::peer::DISCONNECTED::reason(record).map(ToOwned::to_owned);
        self.last_conn_id = record.conn_id();
        self.updated_at = record.at;
    }

    pub fn update_rtt(&mut self, record: &TelemetryRecord, round_trip_micros: u64) {
        self.last_rtt_micros = Some(round_trip_micros);
        self.last_conn_id = record.conn_id();
        self.updated_at = record.at;
    }
}
