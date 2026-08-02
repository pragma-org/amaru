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

use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use tracing::Level;

use crate::{
    config::Config,
    events::{Message, SystemSample, TelemetryRecord},
    startup::StartupContext,
};

mod record_fields;
mod telemetry_event;

use self::{
    record_fields::RecordFields,
    telemetry_event::{CONSENSUS_TARGET, LEDGER_TARGET, PROTOCOLS_TARGET, TelemetryEvent},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Amaru,
    Cardano,
    Config,
}

impl Page {
    pub const ALL: [Self; 3] = [Self::Amaru, Self::Cardano, Self::Config];

    pub fn next(self) -> Self {
        match self {
            Self::Amaru => Self::Cardano,
            Self::Cardano => Self::Config,
            Self::Config => Self::Amaru,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Amaru => Self::Config,
            Self::Cardano => Self::Amaru,
            Self::Config => Self::Cardano,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Amaru => "Amaru",
            Self::Cardano => "Cardano",
            Self::Config => "Config",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Amaru => 0,
            Self::Cardano => 1,
            Self::Config => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPaneMode {
    Normal,
    Maximized,
}

impl LogPaneMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::Normal => Self::Maximized,
            Self::Maximized => Self::Normal,
        }
    }

    pub fn is_maximized(self) -> bool {
        matches!(self, Self::Maximized)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelFilter {
    All,
    Error,
    Warn,
    Info,
    Debug,
}

impl LevelFilter {
    pub const ALL: [Self; 5] = [Self::All, Self::Error, Self::Warn, Self::Info, Self::Debug];

    pub fn allows(self, level: Level) -> bool {
        match self {
            Self::All => true,
            Self::Error => level == Level::ERROR,
            Self::Warn => matches!(level, Level::WARN | Level::ERROR),
            Self::Info => matches!(level, Level::INFO | Level::WARN | Level::ERROR),
            Self::Debug => matches!(level, Level::DEBUG | Level::INFO | Level::WARN | Level::ERROR),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Error => "error",
            Self::Warn => "warn+",
            Self::Info => "info+",
            Self::Debug => "debug+",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFilter {
    All,
    Ledger,
    Consensus,
    Protocols,
    Other,
}

impl TargetFilter {
    pub const ALL: [Self; 5] = [Self::All, Self::Ledger, Self::Consensus, Self::Protocols, Self::Other];

    pub fn allows(self, target: &str) -> bool {
        match self {
            Self::All => true,
            Self::Ledger => target == LEDGER_TARGET,
            Self::Consensus => target == CONSENSUS_TARGET,
            Self::Protocols => target == PROTOCOLS_TARGET,
            Self::Other => !matches!(target, LEDGER_TARGET | CONSENSUS_TARGET | PROTOCOLS_TARGET),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Ledger => "ledger",
            Self::Consensus => "consensus",
            Self::Protocols => "protocols",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TipState {
    pub slot: u64,
    pub header_hash: String,
    pub block_height: u64,
    pub epoch: u64,
    pub slot_in_epoch: u64,
    pub density: f64,
    pub current_kes_period: u64,
    pub remaining_kes_periods: u64,
    pub updated_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StakeSnapshotState {
    pub accounts: usize,
    pub pools: usize,
    pub dreps: usize,
    pub active_stake: u64,
    pub pools_voting_stake: u64,
    pub dreps_voting_stake: u64,
    pub updated_at: Instant,
}

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

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalActivity {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub detail: Option<String>,
    pub seen_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GovernanceSummary {
    pub proposal_count_in_scope: Option<u64>,
    pub dormant_epochs: Option<u64>,
    pub latest_ratification: Option<String>,
}

impl TipState {
    fn from_record(record: RecordFields<'_>) -> Option<Self> {
        Some(Self {
            slot: record.as_u64("slot")?,
            header_hash: record.as_str("header_hash")?.to_owned(),
            block_height: record.as_u64("block_height")?,
            epoch: record.as_u64("epoch")?,
            slot_in_epoch: record.as_u64("slot_in_epoch")?,
            density: record.as_f64("density")?,
            current_kes_period: record.as_u64("current_kes_period")?,
            remaining_kes_periods: record.as_u64("remaining_kes_periods")?,
            updated_at: record.at(),
        })
    }
}

impl StakeSnapshotState {
    fn from_record(record: RecordFields<'_>) -> Option<Self> {
        Some(Self {
            accounts: record.as_u64("accounts")? as usize,
            pools: record.as_u64("pools")? as usize,
            dreps: record.as_u64("dreps")? as usize,
            active_stake: record.as_u64("active_stake")?,
            pools_voting_stake: record.as_u64("pools_voting_stake")?,
            dreps_voting_stake: record.as_u64("dreps_voting_stake")?,
            updated_at: record.at(),
        })
    }
}

impl PeerState {
    fn new(address: String, trusted: bool, updated_at: Instant) -> Self {
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

    fn mark_connected(&mut self, record: RecordFields<'_>) {
        let direction = record.as_str("direction");
        self.connected = true;
        self.inbound |= matches!(direction, Some("Inbound"));
        self.outbound |= matches!(direction, Some("Outbound"));
        self.last_conn_id = record.conn_id();
        self.full_duplex = record.as_bool("full_duplex");
        self.full_duplex_capable = record.as_bool("full_duplex_capable");
        self.last_reason = None;
        self.updated_at = record.at();
    }

    fn mark_disconnected(&mut self, record: RecordFields<'_>) {
        self.connected = false;
        self.last_reason = record.as_str("reason").map(ToOwned::to_owned);
        self.last_conn_id = record.conn_id();
        self.updated_at = record.at();
    }

    fn update_rtt(&mut self, record: RecordFields<'_>, round_trip_micros: u64) {
        self.last_rtt_micros = Some(round_trip_micros);
        self.last_conn_id = record.conn_id();
        self.updated_at = record.at();
    }
}

impl ProposalActivity {
    fn from_record(record: RecordFields<'_>, status: &str, detail: Option<String>) -> Self {
        let id = record.as_str("proposal_id").or_else(|| record.as_str("id")).unwrap_or("unknown").to_owned();
        let kind =
            record.as_str("proposal_kind").or_else(|| record.as_str("proposal_type")).unwrap_or("unknown").to_owned();

        Self { id, kind, status: status.to_owned(), detail, seen_at: record.at() }
    }
}

#[derive(Debug)]
pub struct Model {
    pub startup: StartupContext,
    pub page: Page,
    pub log_pane_mode: LogPaneMode,
    pub level_filter: LevelFilter,
    pub target_filter: TargetFilter,
    pub selected_window: usize,
    pub log_scroll: usize,
    pub peer_scroll: usize,
    pub proposal_scroll: usize,
    pub created_at: Instant,
    pub tip: Option<TipState>,
    pub stake_snapshot: Option<StakeSnapshotState>,
    pub treasury: Option<u64>,
    pub reserves: Option<u64>,
    pub protocol_version: String,
    pub governance: GovernanceSummary,
    pub peers: BTreeMap<String, PeerState>,
    pub logs: VecDeque<TelemetryRecord>,
    pub dropped_logs: u64,
    pub system_samples: VecDeque<SystemSample>,
    pub recent_blocks: VecDeque<Instant>,
    pub recent_peer_events: VecDeque<Instant>,
    pub recent_proposals: VecDeque<ProposalActivity>,
    config: Config,
}

impl Model {
    pub fn new(config: Config, startup: StartupContext) -> Self {
        Self {
            protocol_version: startup.protocol_version.clone(),
            startup,
            page: Page::Amaru,
            log_pane_mode: LogPaneMode::Normal,
            level_filter: LevelFilter::All,
            target_filter: TargetFilter::All,
            selected_window: 0,
            log_scroll: 0,
            peer_scroll: 0,
            proposal_scroll: 0,
            created_at: Instant::now(),
            tip: None,
            stake_snapshot: None,
            treasury: None,
            reserves: None,
            governance: GovernanceSummary::default(),
            peers: BTreeMap::default(),
            logs: VecDeque::default(),
            dropped_logs: 0,
            system_samples: VecDeque::default(),
            recent_blocks: VecDeque::default(),
            recent_peer_events: VecDeque::default(),
            recent_proposals: VecDeque::default(),
            config,
        }
    }

    pub fn windows(&self) -> &[Duration] {
        &self.config.windows
    }

    pub fn current_window(&self) -> Duration {
        self.config.windows[self.selected_window]
    }

    pub fn window_label(&self) -> String {
        crate::config::format_duration_short(self.current_window())
    }

    pub fn is_ready(&self, now: Instant) -> bool {
        self.tip.is_some()
            || self.stake_snapshot.is_some()
            || now.duration_since(self.created_at) >= self.config.splash_timeout
    }

    pub fn handle_message(&mut self, message: Message) {
        match message {
            Message::Telemetry(record) => self.record_telemetry(record),
            Message::DroppedTelemetry => self.dropped_logs = self.dropped_logs.saturating_add(1),
        }
    }

    pub fn push_system_sample(&mut self, sample: SystemSample) {
        self.system_samples.push_back(sample);
        while self.system_samples.len() > self.system_capacity() {
            self.system_samples.pop_front();
        }
    }

    pub fn next_page(&mut self) {
        self.page = self.page.next();
    }

    pub fn previous_page(&mut self) {
        self.page = self.page.previous();
    }

    pub fn set_page(&mut self, page: Page) {
        self.page = page;
    }

    pub fn cycle_log_pane(&mut self) {
        self.log_pane_mode = self.log_pane_mode.toggle();
    }

    pub fn set_window(&mut self, index: usize) {
        if index < self.config.windows.len() {
            self.selected_window = index;
        }
    }

    pub fn set_level_filter(&mut self, level: LevelFilter) {
        self.level_filter = level;
        self.log_scroll = 0;
    }

    pub fn set_target_filter(&mut self, filter: TargetFilter) {
        self.target_filter = filter;
        self.log_scroll = 0;
    }

    pub fn scroll_logs(&mut self, delta: isize) {
        if delta.is_negative() {
            self.log_scroll = self.log_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.log_scroll = self.log_scroll.saturating_add(delta as usize);
        }
    }

    pub fn scroll_peers(&mut self, delta: isize) {
        if delta.is_negative() {
            self.peer_scroll = self.peer_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.peer_scroll = self.peer_scroll.saturating_add(delta as usize);
        }
    }

    pub fn scroll_proposals(&mut self, delta: isize) {
        if delta.is_negative() {
            self.proposal_scroll = self.proposal_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.proposal_scroll = self.proposal_scroll.saturating_add(delta as usize);
        }
    }

    pub fn filtered_logs(&self) -> Vec<&TelemetryRecord> {
        self.logs
            .iter()
            .filter(|record| self.level_filter.allows(record.level) && self.target_filter.allows(&record.target))
            .collect()
    }

    pub fn connected_peer_count(&self) -> usize {
        self.peers.values().filter(|peer| peer.connected).count()
    }

    pub fn inbound_peer_count(&self) -> usize {
        self.peers.values().filter(|peer| peer.connected && peer.inbound).count()
    }

    pub fn outbound_peer_count(&self) -> usize {
        self.peers.values().filter(|peer| peer.connected && peer.outbound).count()
    }

    pub fn average_rtt_millis(&self) -> Option<f64> {
        let (count, total) = self
            .peers
            .values()
            .filter_map(|peer| peer.last_rtt_micros)
            .fold((0_u64, 0_u64), |(count, total), micros| (count + 1, total + micros));

        (count > 0).then_some(total as f64 / count as f64 / 1_000.0)
    }

    pub fn blocks_in_window(&self, now: Instant) -> usize {
        self.recent_blocks.iter().filter(|at| now.duration_since(**at) <= self.current_window()).count()
    }

    pub fn last_block_elapsed(&self, now: Instant) -> Option<Duration> {
        self.tip.as_ref().map(|tip| now.duration_since(tip.updated_at))
    }

    fn record_telemetry(&mut self, record: TelemetryRecord) {
        self.update_state(TelemetryEvent::from_record(&record), RecordFields::from(&record));
        self.logs.push_back(record);
        while self.logs.len() > self.config.log_capacity {
            self.logs.pop_front();
        }
    }

    fn update_state(&mut self, event: Option<TelemetryEvent>, record: RecordFields<'_>) {
        let Some(event) = event else {
            return;
        };

        match event {
            TelemetryEvent::TipUpdate => self.update_tip(record),
            TelemetryEvent::StakeSnapshot => self.update_stake_snapshot(record),
            TelemetryEvent::RewardsSummarize | TelemetryEvent::BootstrapPotsImport => self.update_pots(record),
            TelemetryEvent::KeepaliveRoundTrip => self.update_peer_rtt(record),
            TelemetryEvent::PeerConnected => self.update_peer_connected(record),
            TelemetryEvent::PeerDisconnected => self.update_peer_disconnected(record),
            TelemetryEvent::GovernanceActivityUpdate => {
                self.governance.dormant_epochs = record.as_u64("consecutive_dormant_epochs");
            }
            TelemetryEvent::NewGovernanceUpdates => {
                self.governance.proposal_count_in_scope = record.as_u64("proposals_count");
            }
            TelemetryEvent::GovernanceRatifying => self.push_proposal(record, "ratifying", None),
            TelemetryEvent::GovernanceEnacting => self.push_proposal(record, "enacting", None),
            TelemetryEvent::ProposalDrop => {
                let detail = record.as_str("expired").map(|expired| format!("expired={expired}"));
                self.push_proposal(record, "dropped", detail);
            }
            TelemetryEvent::ProposalSkip => {
                self.push_proposal(record, "skipped", record.as_str("reason").map(ToOwned::to_owned));
            }
            TelemetryEvent::ProtocolUpgrade => {
                if let Some(new_version) = record.as_str("new_version").map(ToOwned::to_owned) {
                    self.protocol_version = new_version;
                }
            }
            TelemetryEvent::ProtocolParametersRatify => {
                if let Some(version) = record.as_str("protocol_version").map(ToOwned::to_owned) {
                    self.protocol_version = version;
                }
            }
            TelemetryEvent::RatificationSummarize => {
                self.governance.latest_ratification = Some(record.to_fields_string());
            }
        }
    }

    fn update_tip(&mut self, record: RecordFields<'_>) {
        let Some(tip) = TipState::from_record(record) else {
            return;
        };

        self.tip = Some(tip);
        self.push_recent_block(record.at());
    }

    fn update_stake_snapshot(&mut self, record: RecordFields<'_>) {
        self.stake_snapshot = StakeSnapshotState::from_record(record);
    }

    fn update_pots(&mut self, record: RecordFields<'_>) {
        self.treasury = record.as_u64("pots_treasury").or_else(|| record.as_u64("treasury"));
        self.reserves = record.as_u64("pots_reserves").or_else(|| record.as_u64("reserves"));
    }

    fn update_peer_connected(&mut self, record: RecordFields<'_>) {
        let Some(address) = record.as_str("peer") else {
            return;
        };
        let peer = self.peer_mut(address, record.at());
        peer.mark_connected(record);
        self.push_recent_peer_event(record.at());
    }

    fn update_peer_disconnected(&mut self, record: RecordFields<'_>) {
        let Some(address) = record.as_str("peer") else {
            return;
        };
        if let Some(peer) = self.peers.get_mut(address) {
            peer.mark_disconnected(record);
        }

        self.push_recent_peer_event(record.at());
    }

    fn update_peer_rtt(&mut self, record: RecordFields<'_>) {
        let Some(address) = record.as_str("peer") else {
            return;
        };
        let Some(round_trip_micros) = record.as_u64("round_trip_micros") else {
            return;
        };

        let peer = self.peer_mut(address, record.at());
        peer.connected = true;
        peer.update_rtt(record, round_trip_micros);
    }

    fn push_proposal(&mut self, record: RecordFields<'_>, status: &str, detail: Option<String>) {
        self.recent_proposals.push_front(ProposalActivity::from_record(record, status, detail));

        while self.recent_proposals.len() > self.config.proposal_capacity {
            self.recent_proposals.pop_back();
        }
    }

    fn peer_mut(&mut self, address: &str, updated_at: Instant) -> &mut PeerState {
        let trusted = self.startup.trusted_peers.contains(address);
        self.peers.entry(address.to_owned()).or_insert_with(|| PeerState::new(address.to_owned(), trusted, updated_at))
    }

    fn push_recent_block(&mut self, at: Instant) {
        let max_window = self.max_window();
        self.recent_blocks.push_back(at);
        prune_recent(&mut self.recent_blocks, at, max_window);
    }

    fn push_recent_peer_event(&mut self, at: Instant) {
        let max_window = self.max_window();
        self.recent_peer_events.push_back(at);
        prune_recent(&mut self.recent_peer_events, at, max_window);
    }

    fn max_window(&self) -> Duration {
        self.config.windows.last().copied().unwrap_or_default()
    }

    fn system_capacity(&self) -> usize {
        let max_window = self.max_window().as_secs();
        let sample_interval = self.config.sample_interval.as_secs().max(1);
        (max_window / sample_interval).max(1) as usize + 2
    }
}

fn prune_recent(entries: &mut VecDeque<Instant>, now: Instant, max_window: Duration) {
    while entries.front().is_some_and(|at| now.duration_since(*at) > max_window) {
        entries.pop_front();
    }
}

pub fn render_fields(record: &TelemetryRecord) -> String {
    RecordFields::from(record).to_fields_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{
        events::{FieldValue, TelemetryKind, TelemetryRecord},
        startup::ProcessInfo,
    };

    fn telemetry(target: &str, name: &str, fields: &[(&str, FieldValue)]) -> TelemetryRecord {
        TelemetryRecord {
            kind: TelemetryKind::Event,
            level: Level::INFO,
            target: target.into(),
            name: name.into(),
            at: Instant::now(),
            wall_time: std::time::SystemTime::UNIX_EPOCH,
            fields: fields.iter().map(|(name, value)| ((*name).into(), value.clone())).collect(),
        }
    }

    #[test]
    fn updates_tip_from_public_event() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            system_start_millis: 1_666_656_000_000,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "tip.update",
            &[
                ("slot", FieldValue::U64(1)),
                ("header_hash", FieldValue::String("abc".into())),
                ("block_height", FieldValue::U64(2)),
                ("epoch", FieldValue::U64(3)),
                ("slot_in_epoch", FieldValue::U64(4)),
                ("density", FieldValue::F64(0.5)),
                ("current_kes_period", FieldValue::U64(5)),
                ("remaining_kes_periods", FieldValue::U64(6)),
            ],
        )));

        let tip = model.tip.expect("tip must be recorded");
        assert_eq!(tip.slot, 1);
        assert_eq!(tip.epoch, 3);
        assert_eq!(tip.block_height, 2);
    }

    #[test]
    fn updates_peer_rtt() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            system_start_millis: 1_666_656_000_000,
            trusted_peers: BTreeSet::from(["1.2.3.4:3001".into()]),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry(
            PROTOCOLS_TARGET,
            "keepalive.peer.round_trip",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("conn_id", FieldValue::String("7".into())),
                ("round_trip_micros", FieldValue::U64(12_345)),
            ],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.last_rtt_micros, Some(12_345));
        assert!(peer.trusted);
    }
}
