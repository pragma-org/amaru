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
    config::{Config, TimeWindow},
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
pub enum PaneMode {
    Normal,
    Maximized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    Normal,
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollFocus {
    Logs,
    Peers,
    Proposals,
}

impl ScrollFocus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Peers => "peers",
            Self::Proposals => "proposals",
        }
    }
}

impl PaneMode {
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
    Debug,
    Info,
    Warn,
    Error,
}

impl LevelFilter {
    pub const ALL: [Self; 4] = [Self::Debug, Self::Info, Self::Warn, Self::Error];

    pub fn allows(self, level: Level) -> bool {
        match self {
            Self::Debug => matches!(level, Level::DEBUG | Level::INFO | Level::WARN | Level::ERROR),
            Self::Info => matches!(level, Level::INFO | Level::WARN | Level::ERROR),
            Self::Warn => matches!(level, Level::WARN | Level::ERROR),
            Self::Error => level == Level::ERROR,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
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
pub struct MempoolState {
    pub tx_count: u64,
    pub size_bytes: u64,
    pub updated_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitialStakeDistributionState {
    pub epoch: u64,
    pub progress: f64,
    pub completed: bool,
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
    pub proposed_in: Option<u64>,
    pub valid_until: Option<u64>,
    pub constitutional_committee: Option<bool>,
    pub delegate_representatives: Option<bool>,
    pub stake_pool_operators: Option<bool>,
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
        let mut proposal = Self {
            id: proposal_id(record).unwrap_or_else(|| "unknown".to_owned()),
            kind: proposal_kind(record).unwrap_or("unknown").to_owned(),
            status: status.to_owned(),
            detail: None,
            proposed_in: None,
            valid_until: None,
            constitutional_committee: None,
            delegate_representatives: None,
            stake_pool_operators: None,
            seen_at: record.at(),
        };
        proposal.merge_from_record(record, status, detail);
        proposal
    }

    fn merge_from_record(&mut self, record: RecordFields<'_>, status: &str, detail: Option<String>) {
        if let Some(kind) = proposal_kind(record) {
            self.kind = kind.to_owned();
        }
        self.status = status.to_owned();
        if let Some(detail) = detail {
            self.detail = Some(detail);
        }
        if let Some(proposed_in) = record.as_u64("proposed_in") {
            self.proposed_in = Some(proposed_in);
        }
        if let Some(valid_until) = record.as_u64("valid_until") {
            self.valid_until = Some(valid_until);
        }
        if let Some(approved) = record.as_bool("approved_by_constitutional_committee") {
            self.constitutional_committee = Some(approved);
        }
        if let Some(approved) = record.as_bool("approved_by_dreps") {
            self.delegate_representatives = Some(approved);
        }
        if let Some(approved) = record.as_bool("approved_by_pools") {
            self.stake_pool_operators = Some(approved);
        }
        self.seen_at = record.at();
    }
}

#[derive(Debug)]
pub struct Model {
    pub startup: StartupContext,
    pub page: Page,
    pub interaction_mode: InteractionMode,
    pub log_pane_mode: PaneMode,
    pub peer_pane_mode: PaneMode,
    pub proposal_pane_mode: PaneMode,
    pub scroll_focus: ScrollFocus,
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
    pub fees: Option<u64>,
    pub donations: Option<u64>,
    pub mempool: MempoolState,
    pub protocol_version: String,
    pub governance: GovernanceSummary,
    pub epoch_overlay_exists: bool,
    pub rewards_ready: bool,
    pub peers: BTreeMap<String, PeerState>,
    pub logs: VecDeque<TelemetryRecord>,
    pub system_samples: VecDeque<SystemSample>,
    pub recent_blocks: VecDeque<Instant>,
    pub recent_transactions: VecDeque<(Instant, u64)>,
    pub recent_rollbacks: VecDeque<(Instant, usize)>,
    pub initial_stake_distribution_order: Vec<u64>,
    pub initial_stake_distributions: BTreeMap<u64, InitialStakeDistributionState>,
    pub initial_stake_distributions_ready: bool,
    pub proposal_order: VecDeque<String>,
    pub proposals_by_id: BTreeMap<String, ProposalActivity>,
    config: Config,
}

impl Model {
    pub fn new(config: Config, startup: StartupContext) -> Self {
        Self {
            protocol_version: startup.protocol_version.clone(),
            startup,
            page: Page::Amaru,
            interaction_mode: InteractionMode::Normal,
            log_pane_mode: PaneMode::Normal,
            peer_pane_mode: PaneMode::Normal,
            proposal_pane_mode: PaneMode::Normal,
            scroll_focus: ScrollFocus::Logs,
            level_filter: LevelFilter::Debug,
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
            fees: None,
            donations: None,
            mempool: MempoolState { tx_count: 0, size_bytes: 0, updated_at: Instant::now() },
            governance: GovernanceSummary::default(),
            epoch_overlay_exists: false,
            rewards_ready: false,
            peers: BTreeMap::default(),
            logs: VecDeque::default(),
            system_samples: VecDeque::default(),
            recent_blocks: VecDeque::default(),
            recent_transactions: VecDeque::default(),
            recent_rollbacks: VecDeque::default(),
            initial_stake_distribution_order: Vec::default(),
            initial_stake_distributions: BTreeMap::default(),
            initial_stake_distributions_ready: false,
            proposal_order: VecDeque::default(),
            proposals_by_id: BTreeMap::default(),
            config,
        }
    }

    pub fn windows(&self) -> &[TimeWindow] {
        &self.config.windows
    }

    pub fn current_window(&self) -> Duration {
        self.config.windows[self.selected_window].as_duration()
    }

    pub fn is_ready(&self, now: Instant) -> bool {
        if self.initial_stake_distributions_ready {
            return true;
        }

        if !self.initial_stake_distribution_order.is_empty() {
            return false;
        }

        self.tip.is_some()
            || self.stake_snapshot.is_some()
            || now.duration_since(self.created_at) >= self.config.splash_timeout
    }

    pub fn initial_stake_distributions(&self) -> impl Iterator<Item = &InitialStakeDistributionState> {
        self.initial_stake_distribution_order.iter().filter_map(|epoch| self.initial_stake_distributions.get(epoch))
    }

    pub fn handle_message(&mut self, message: Message) {
        match message {
            Message::Telemetry(record) => self.record_telemetry(record),
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

    pub fn enter_copy_mode(&mut self) {
        self.interaction_mode = InteractionMode::Copy;
    }

    pub fn exit_copy_mode(&mut self) {
        self.interaction_mode = InteractionMode::Normal;
    }

    pub fn is_copy_mode(&self) -> bool {
        self.interaction_mode == InteractionMode::Copy
    }

    pub fn cycle_log_pane(&mut self) {
        self.log_pane_mode = self.log_pane_mode.toggle();
        if self.log_pane_mode.is_maximized() {
            self.peer_pane_mode = PaneMode::Normal;
            self.proposal_pane_mode = PaneMode::Normal;
        }
    }

    pub fn cycle_peer_pane(&mut self) {
        self.peer_pane_mode = self.peer_pane_mode.toggle();
        if self.peer_pane_mode.is_maximized() {
            self.log_pane_mode = PaneMode::Normal;
            self.proposal_pane_mode = PaneMode::Normal;
        }
    }

    pub fn cycle_proposal_pane(&mut self) {
        self.proposal_pane_mode = self.proposal_pane_mode.toggle();
        if self.proposal_pane_mode.is_maximized() {
            self.log_pane_mode = PaneMode::Normal;
            self.peer_pane_mode = PaneMode::Normal;
        }
    }

    pub fn next_scroll_focus(&mut self) {
        self.scroll_focus = match (self.page, self.scroll_focus) {
            (Page::Amaru, ScrollFocus::Logs) => ScrollFocus::Peers,
            (Page::Amaru, ScrollFocus::Peers) => ScrollFocus::Logs,
            (Page::Amaru, ScrollFocus::Proposals) => ScrollFocus::Logs,
            (Page::Cardano, ScrollFocus::Logs) => ScrollFocus::Proposals,
            (Page::Cardano, ScrollFocus::Proposals) => ScrollFocus::Logs,
            (Page::Cardano, ScrollFocus::Peers) => ScrollFocus::Logs,
            (Page::Config, focus) => focus,
        };
    }

    pub fn set_window(&mut self, index: usize) {
        if index < self.config.windows.len() {
            self.selected_window = index;
        }
    }

    pub fn set_level_filter(&mut self, level: LevelFilter) {
        self.level_filter = level;
        self.log_scroll = 0;
        self.scroll_focus = ScrollFocus::Logs;
    }

    pub fn set_target_filter(&mut self, filter: TargetFilter) {
        self.target_filter = filter;
        self.log_scroll = 0;
        self.scroll_focus = ScrollFocus::Logs;
    }

    pub fn focus_logs(&mut self) {
        self.scroll_focus = ScrollFocus::Logs;
    }

    pub fn focus_peers(&mut self) {
        self.scroll_focus = ScrollFocus::Peers;
    }

    pub fn focus_proposals(&mut self) {
        self.scroll_focus = ScrollFocus::Proposals;
    }

    pub fn scroll_focused(&mut self, delta: isize) {
        match self.scroll_focus {
            ScrollFocus::Logs => self.scroll_logs(delta),
            ScrollFocus::Peers => self.scroll_peers(delta),
            ScrollFocus::Proposals => self.scroll_proposals(delta),
        }
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

    pub fn blocks_in_window(&self, now: Instant) -> usize {
        self.recent_blocks.iter().filter(|at| now.duration_since(**at) <= self.current_window()).count()
    }

    pub fn average_rtt_millis(&self) -> Option<f64> {
        let (count, total) = self
            .peers
            .values()
            .filter(|peer| peer.connected)
            .filter_map(|peer| peer.last_rtt_micros)
            .fold((0_u64, 0_u64), |(count, total), micros| (count + 1, total + micros));

        (count > 0).then_some(total as f64 / count as f64 / 1_000.0)
    }

    pub fn inbound_peer_count(&self) -> usize {
        self.peers.values().filter(|peer| peer.connected && peer.inbound).count()
    }

    pub fn outbound_peer_count(&self) -> usize {
        self.peers.values().filter(|peer| peer.connected && peer.outbound).count()
    }

    pub fn last_block_elapsed(&self, now: Instant) -> Option<Duration> {
        self.tip.as_ref().map(|tip| now.duration_since(tip.updated_at))
    }

    pub fn transactions_in_window(&self, now: Instant) -> u64 {
        self.recent_transactions
            .iter()
            .filter(|(at, _)| now.duration_since(*at) <= self.current_window())
            .map(|(_, count)| *count)
            .sum()
    }

    pub fn average_rollback_length(&self, now: Instant) -> Option<f64> {
        let (count, total) = self
            .recent_rollbacks
            .iter()
            .filter(|(at, _)| now.duration_since(*at) <= self.current_window())
            .fold((0_u64, 0_u64), |(count, total), (_, length)| (count + 1, total + *length as u64));

        (count > 0).then_some(total as f64 / count as f64)
    }

    pub fn proposals(&self) -> impl Iterator<Item = &ProposalActivity> {
        self.proposal_order.iter().filter_map(|id| self.proposals_by_id.get(id))
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
            TelemetryEvent::MempoolStateUpdate => self.update_mempool(record),
            TelemetryEvent::StakeDistributionInitialBegin => self.begin_initial_stake_distribution(record),
            TelemetryEvent::StakeDistributionInitialProgress => self.advance_initial_stake_distribution(record),
            TelemetryEvent::StakeDistributionInitialReady => self.complete_initial_stake_distributions(record.at()),
            TelemetryEvent::RewardsSummarize => {
                self.update_pots(record);
                self.rewards_ready = true;
            }
            TelemetryEvent::BootstrapPotsImport | TelemetryEvent::PotsLoad => self.update_pots(record),
            TelemetryEvent::EpochTransitionCompute => self.rewards_ready = false,
            TelemetryEvent::EpochTransitionRecord => self.epoch_overlay_exists = true,
            TelemetryEvent::EpochTransitionApply => self.epoch_overlay_exists = false,
            TelemetryEvent::StateSwitchToFork => self.push_recent_rollback(record),
            TelemetryEvent::KeepaliveRoundTrip => self.update_peer_rtt(record),
            TelemetryEvent::PeerConnected => self.update_peer_connected(record),
            TelemetryEvent::PeerDisconnected => self.update_peer_disconnected(record),
            TelemetryEvent::GovernanceActivityUpdate => {
                self.governance.dormant_epochs = record.as_u64("consecutive_dormant_epochs");
            }
            TelemetryEvent::NewGovernanceUpdates => {
                self.governance.proposal_count_in_scope = record.as_u64("proposals_count");
            }
            TelemetryEvent::GovernanceRatifying => self.upsert_proposal(record, "ratifying", None),
            TelemetryEvent::GovernanceEnacting => self.upsert_proposal(record, "enacted", None),
            TelemetryEvent::ProposalActive => {
                self.push_active_proposal(record);
            }
            TelemetryEvent::ProposalDrop => {
                self.push_dropped_proposal(record);
            }
            TelemetryEvent::ProposalSkip => {
                self.upsert_proposal(record, "skipped", record.as_str("reason").map(ToOwned::to_owned))
            }
            TelemetryEvent::ProtocolUpgrade => {
                if let Some(new_version) = record.as_str("new_version").map(ToOwned::to_owned) {
                    self.protocol_version = new_version;
                }
            }
            TelemetryEvent::ProtocolParametersLoad | TelemetryEvent::ProtocolParametersRatify => {
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
        self.push_recent_transactions(record);
    }

    fn update_stake_snapshot(&mut self, record: RecordFields<'_>) {
        self.stake_snapshot = StakeSnapshotState::from_record(record);
    }

    fn update_mempool(&mut self, record: RecordFields<'_>) {
        let Some(tx_count) = record.as_u64("tx_count") else {
            return;
        };
        let Some(size_bytes) = record.as_u64("size_bytes") else {
            return;
        };

        self.mempool = MempoolState { tx_count, size_bytes, updated_at: record.at() };
    }

    fn begin_initial_stake_distribution(&mut self, record: RecordFields<'_>) {
        let Some(epoch) = record.as_u64("epoch") else {
            return;
        };

        self.initial_stake_distributions_ready = false;
        if !self.initial_stake_distribution_order.contains(&epoch) {
            self.initial_stake_distribution_order.push(epoch);
            self.initial_stake_distribution_order.sort_unstable();
        }

        self.initial_stake_distributions.entry(epoch).or_insert(InitialStakeDistributionState {
            epoch,
            progress: 0.0,
            completed: false,
            updated_at: record.at(),
        });
    }

    fn advance_initial_stake_distribution(&mut self, record: RecordFields<'_>) {
        let Some(epoch) = record.as_u64("epoch") else {
            return;
        };
        let Some(progress) = record.as_f64("progress") else {
            return;
        };

        self.begin_initial_stake_distribution(record);
        if let Some(state) = self.initial_stake_distributions.get_mut(&epoch) {
            state.progress = progress.clamp(0.0, 1.0);
            state.updated_at = record.at();
        }
    }

    fn complete_initial_stake_distributions(&mut self, at: Instant) {
        self.initial_stake_distributions_ready = true;

        for state in self.initial_stake_distributions.values_mut() {
            state.progress = 1.0;
            state.completed = true;
            state.updated_at = at;
        }
    }

    fn push_recent_transactions(&mut self, record: RecordFields<'_>) {
        let Some(tx_count) = record.as_u64("tx_count") else {
            return;
        };

        let at = record.at();
        let max_window = self.max_window();
        self.recent_transactions.push_back((at, tx_count));

        while self.recent_transactions.front().is_some_and(|(entry_at, _)| at.duration_since(*entry_at) > max_window) {
            self.recent_transactions.pop_front();
        }
    }

    fn push_active_proposal(&mut self, record: RecordFields<'_>) {
        self.upsert_proposal(record, "-", record.as_str("detail").map(ToOwned::to_owned));
        self.governance.proposal_count_in_scope = Some(self.governance.proposal_count_in_scope.unwrap_or_default() + 1);
    }

    fn push_dropped_proposal(&mut self, record: RecordFields<'_>) {
        let status = if proposal_id(record)
            .and_then(|id| self.proposals_by_id.get(&id))
            .is_some_and(|proposal| proposal.status == "enacted")
        {
            "enacted"
        } else if record.as_bool("expired").unwrap_or(false) {
            "expired"
        } else {
            "dropped"
        };

        let detail = if status == "expired" {
            Some("expired".to_string())
        } else if record.as_bool("ratified_or_evicted").unwrap_or(false) {
            Some("superseded".to_string())
        } else {
            None
        };
        self.upsert_proposal(record, status, detail);
        if let Some(count) = self.governance.proposal_count_in_scope.as_mut() {
            *count = count.saturating_sub(1);
        }
    }

    fn update_pots(&mut self, record: RecordFields<'_>) {
        self.treasury = record.as_u64("pots_treasury").or_else(|| record.as_u64("treasury"));
        self.reserves = record.as_u64("pots_reserves").or_else(|| record.as_u64("reserves"));
        self.fees = record.as_u64("pots_fees").or_else(|| record.as_u64("fees"));
        self.donations = record.as_u64("pots_donations").or_else(|| record.as_u64("donations"));
    }

    fn update_peer_connected(&mut self, record: RecordFields<'_>) {
        let Some(address) = record.as_str("peer") else {
            return;
        };
        let peer = self.peer_mut(address, record.at());
        peer.mark_connected(record);
    }

    fn update_peer_disconnected(&mut self, record: RecordFields<'_>) {
        let Some(address) = record.as_str("peer") else {
            return;
        };
        if let Some(peer) = self.peers.get_mut(address) {
            peer.mark_disconnected(record);
        }
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
        if !peer.inbound && !peer.outbound {
            peer.outbound = true;
        }
        peer.update_rtt(record, round_trip_micros);
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

    fn push_recent_rollback(&mut self, record: RecordFields<'_>) {
        let Some(rollback_length) = record.as_u64("rollback_length") else {
            return;
        };

        let max_window = self.max_window();
        let at = record.at();
        self.recent_rollbacks.push_back((at, rollback_length as usize));

        while self.recent_rollbacks.front().is_some_and(|(entry_at, _)| at.duration_since(*entry_at) > max_window) {
            self.recent_rollbacks.pop_front();
        }
    }

    fn upsert_proposal(&mut self, record: RecordFields<'_>, status: &str, detail: Option<String>) {
        let Some(id) = proposal_id(record) else {
            return;
        };

        if let Some(proposal) = self.proposals_by_id.get_mut(&id) {
            proposal.merge_from_record(record, status, detail);
        } else {
            self.proposals_by_id.insert(id.clone(), ProposalActivity::from_record(record, status, detail));
        }

        self.proposal_order.retain(|existing| existing != &id);
        self.proposal_order.push_front(id.clone());

        while self.proposal_order.len() > self.config.proposal_capacity {
            if let Some(removed) = self.proposal_order.pop_back() {
                self.proposals_by_id.remove(&removed);
            }
        }
    }

    fn max_window(&self) -> Duration {
        self.config.windows.last().copied().map(TimeWindow::as_duration).unwrap_or_default()
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

fn proposal_id(record: RecordFields<'_>) -> Option<String> {
    record.as_str("proposal_id").or_else(|| record.as_str("id")).map(ToOwned::to_owned)
}

fn proposal_kind(record: RecordFields<'_>) -> Option<&str> {
    record.as_str("proposal_kind").or_else(|| record.as_str("proposal_type"))
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
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
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
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::from(["1.2.3.4:3001".into()]),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
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

    #[test]
    fn waits_for_initial_stake_distributions_ready_event() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);
        let later = model.created_at + Duration::from_secs(60);

        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "stake_distribution.initial_begin",
            &[("epoch", FieldValue::U64(100))],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "stake_distribution.initial_progress",
            &[("epoch", FieldValue::U64(100)), ("progress", FieldValue::F64(0.42))],
        )));

        assert!(!model.is_ready(later));
        assert_eq!(model.initial_stake_distributions().count(), 1);
        assert_eq!(model.initial_stake_distributions().next().map(|state| state.progress), Some(0.42));

        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "stake_distribution.initial_ready",
            &[("epochs", FieldValue::String("100".into()))],
        )));

        assert!(model.is_ready(later));
        assert_eq!(model.initial_stake_distributions().next().map(|state| state.progress), Some(1.0));
        assert_eq!(model.initial_stake_distributions().next().map(|state| state.completed), Some(true));
    }

    #[test]
    fn initial_stake_distributions_are_ordered_by_epoch() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "stake_distribution.initial_begin",
            &[("epoch", FieldValue::U64(101))],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "stake_distribution.initial_begin",
            &[("epoch", FieldValue::U64(99))],
        )));

        let epochs = model.initial_stake_distributions().map(|state| state.epoch).collect::<Vec<_>>();
        assert_eq!(epochs, vec![99, 101]);
    }

    #[test]
    fn proposal_drop_distinguishes_expired_dropped_and_enacted() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "governance.enacting",
            &[
                ("proposal_id", FieldValue::String("enacted".into())),
                ("proposal_kind", FieldValue::String("constitution".into())),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "proposal.drop",
            &[
                ("id", FieldValue::String("enacted".into())),
                ("expired", FieldValue::Bool(false)),
                ("ratified_or_evicted", FieldValue::Bool(true)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "proposal.drop",
            &[
                ("id", FieldValue::String("expired".into())),
                ("proposal_kind", FieldValue::String("hard-fork".into())),
                ("expired", FieldValue::Bool(true)),
                ("ratified_or_evicted", FieldValue::Bool(false)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "proposal.drop",
            &[
                ("id", FieldValue::String("dropped".into())),
                ("proposal_kind", FieldValue::String("treasury-withdrawal".into())),
                ("expired", FieldValue::Bool(false)),
                ("ratified_or_evicted", FieldValue::Bool(true)),
            ],
        )));

        assert_eq!(model.proposals_by_id.get("enacted").map(|proposal| proposal.status.as_str()), Some("enacted"));
        assert_eq!(model.proposals_by_id.get("expired").map(|proposal| proposal.status.as_str()), Some("expired"));
        assert_eq!(model.proposals_by_id.get("dropped").map(|proposal| proposal.status.as_str()), Some("dropped"));
    }

    #[test]
    fn proposal_drop_keeps_enacted_status_even_when_expired_flag_is_set() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "governance.enacting",
            &[
                ("proposal_id", FieldValue::String("proposal".into())),
                ("proposal_kind", FieldValue::String("protocol-parameters".into())),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            LEDGER_TARGET,
            "proposal.drop",
            &[
                ("id", FieldValue::String("proposal".into())),
                ("expired", FieldValue::Bool(true)),
                ("ratified_or_evicted", FieldValue::Bool(true)),
            ],
        )));

        assert_eq!(model.proposals_by_id.get("proposal").map(|proposal| proposal.status.as_str()), Some("enacted"));
    }

    #[test]
    fn keepalive_rtt_marks_peer_as_outbound_when_direction_is_missing() {
        let startup = StartupContext {
            process: ProcessInfo {
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            trusted_peers: BTreeSet::default(),
            runtime_sections: Vec::default(),
            global_sections: Vec::default(),
            protocol_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry(
            PROTOCOLS_TARGET,
            "keepalive.peer.round_trip",
            &[("peer", FieldValue::String("1.2.3.4:3001".into())), ("round_trip_micros", FieldValue::U64(1_000))],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert!(peer.outbound);
        assert!(!peer.inbound);
    }
}
