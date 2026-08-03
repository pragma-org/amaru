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

use amaru_metrics::{LedgerMetrics, MempoolMetrics, MetricsEvent, SystemMetrics};
use amaru_observability::amaru::{bootstrap, consensus, ledger, mempool, protocols};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use crate::{
    config::{Config, TimeWindow},
    events::{Message, MetricRecord, SystemSample, TelemetryRecord},
    startup::StartupContext,
    ui::Views,
};

mod governance_summary;
mod initial_stake_distribution_state;
mod interaction_mode;
mod level_filter;
mod mempool_state;
mod page;
mod pane_mode;
mod peer_state;
mod proposal_activity;
mod scroll_focus;
mod stake_snapshot_state;
mod target_filter;
mod telemetry_event;
mod terminal_event_outcome;
mod tip_state;

use self::{
    governance_summary::GovernanceSummary,
    mempool_state::MempoolState,
    proposal_activity::{ProposalActivity, proposal_id},
    stake_snapshot_state::StakeSnapshotState,
    telemetry_event::TelemetryEvent,
    tip_state::TipState,
};
pub use self::{
    initial_stake_distribution_state::InitialStakeDistributionState, interaction_mode::InteractionMode,
    level_filter::LevelFilter, page::Page, pane_mode::PaneMode, peer_state::PeerState, scroll_focus::ScrollFocus,
    target_filter::TargetFilter, terminal_event_outcome::TerminalEventOutcome,
};

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
    pub config_scroll: usize,
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
            config_scroll: 0,
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
            Message::Metrics(record) => self.record_metrics(record),
        }
    }

    pub fn handle_terminal_event(&mut self, event: Event, views: &Views) -> TerminalEventOutcome {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key_event(key),
            Event::Mouse(mouse) => self.handle_mouse_event(mouse, views),
            Event::Resize(_, _) => TerminalEventOutcome::Continue,
            Event::FocusGained | Event::FocusLost | Event::Paste(_) | Event::Key(_) => TerminalEventOutcome::Continue,
        }
    }

    pub fn push_system_sample(&mut self, sample: SystemSample) {
        self.system_samples.push_back(sample);
        while self.system_samples.len() > self.system_capacity() {
            self.system_samples.pop_front();
        }
    }

    pub fn next_page(&mut self) {
        self.set_page(self.page.next());
    }

    pub fn previous_page(&mut self) {
        self.set_page(self.page.previous());
    }

    pub fn set_page(&mut self, page: Page) {
        self.page = page;
        self.scroll_focus = match self.page {
            Page::Amaru if matches!(self.scroll_focus, ScrollFocus::Logs | ScrollFocus::Peers) => self.scroll_focus,
            Page::Cardano if matches!(self.scroll_focus, ScrollFocus::Logs | ScrollFocus::Proposals) => {
                self.scroll_focus
            }
            Page::Config => ScrollFocus::Config,
            Page::Amaru | Page::Cardano => ScrollFocus::Logs,
        };
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
        self.scroll_focus = self.scroll_focus.next_for(self.page);
    }

    pub fn previous_scroll_focus(&mut self) {
        self.scroll_focus = self.scroll_focus.previous_for(self.page);
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

    pub fn scroll_focused(&mut self, delta: isize) {
        match self.scroll_focus {
            ScrollFocus::Logs => self.scroll_logs(delta),
            ScrollFocus::Peers => self.scroll_peers(delta),
            ScrollFocus::Proposals => self.scroll_proposals(delta),
            ScrollFocus::Config => self.scroll_config(delta),
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

    pub fn scroll_config(&mut self, delta: isize) {
        if delta.is_negative() {
            self.config_scroll = self.config_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.config_scroll = self.config_scroll.saturating_add(delta as usize);
        }
    }

    pub fn handle_click(&mut self, views: &Views, point: Rect) {
        if let Some(page) = views.page_at(point) {
            self.set_page(page);
            return;
        }

        if views.toggles_logs(point) {
            self.cycle_log_pane();
            return;
        }

        if views.toggles_peers(point) {
            self.cycle_peer_pane();
            return;
        }

        if views.toggles_proposals(point) {
            self.cycle_proposal_pane();
            return;
        }

        if let Some(focus) = views.focus_at(point) {
            self.set_scroll_focus(focus);
        }

        if let Some(index) = views.window_at(point) {
            self.set_window(index);
            return;
        }

        if let Some(level) = views.level_filter_at(point) {
            self.set_level_filter(level);
            return;
        }

        if let Some(filter) = views.target_filter_at(point) {
            self.set_target_filter(filter);
        }
    }

    pub fn handle_scroll(&mut self, views: &Views, point: Rect, delta: isize) {
        self.set_scroll_focus(views.scroll_focus_at(point));
        self.scroll_focused(delta);
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

    pub fn rollback_frequency(&self, now: Instant) -> Option<f64> {
        let count =
            self.recent_rollbacks.iter().filter(|(at, _)| now.duration_since(*at) <= self.current_window()).count();

        let window = now.duration_since(self.created_at).max(self.current_window());

        (window > Duration::ZERO).then_some(count as f64 / window.as_secs_f64())
    }

    pub fn proposals(&self) -> impl Iterator<Item = &ProposalActivity> {
        self.proposal_order.iter().filter_map(|id| self.proposals_by_id.get(id))
    }

    pub fn toggle_focused_pane(&mut self) -> bool {
        match (self.page, self.scroll_focus) {
            (Page::Amaru | Page::Cardano, ScrollFocus::Logs) => {
                self.cycle_log_pane();
                true
            }
            (Page::Amaru, ScrollFocus::Peers) => {
                self.cycle_peer_pane();
                true
            }
            (Page::Cardano, ScrollFocus::Proposals) => {
                self.cycle_proposal_pane();
                true
            }
            (Page::Amaru, ScrollFocus::Proposals | ScrollFocus::Config)
            | (Page::Cardano, ScrollFocus::Peers | ScrollFocus::Config)
            | (Page::Config, _) => false,
        }
    }

    fn record_telemetry(&mut self, record: TelemetryRecord) {
        self.update_state(TelemetryEvent::from_record(&record), &record);
        self.logs.push_back(record);
        while self.logs.len() > self.config.log_capacity {
            self.logs.pop_front();
        }
    }

    fn record_metrics(&mut self, record: MetricRecord) {
        match record.event {
            MetricsEvent::LedgerMetrics(metrics) => self.record_ledger_metrics(record.at, metrics),
            MetricsEvent::MempoolMetrics(metrics) => self.record_mempool_metrics(record.at, metrics),
            MetricsEvent::SystemMetrics(metrics) => self.record_system_metrics(record.at, metrics),
            MetricsEvent::ProtocolMetrics(_) | MetricsEvent::ConsensusMetrics(_) => {}
        }
    }

    fn update_state(&mut self, event: Option<TelemetryEvent>, record: &TelemetryRecord) {
        let Some(event) = event else {
            return;
        };

        match event {
            TelemetryEvent::TipUpdate => self.update_tip(record),
            TelemetryEvent::StakeSnapshot => self.update_stake_snapshot(record),
            TelemetryEvent::MempoolStateUpdate => self.update_mempool(record),
            TelemetryEvent::StakeDistributionInitialBegin => self.begin_initial_stake_distribution(record),
            TelemetryEvent::StakeDistributionInitialProgress => self.advance_initial_stake_distribution(record),
            TelemetryEvent::StakeDistributionInitialReady => self.complete_initial_stake_distributions(record.at),
            TelemetryEvent::RewardsSummarize => {
                self.update_pots(record);
                self.rewards_ready = true;
            }
            TelemetryEvent::BootstrapPotsImport | TelemetryEvent::PotsLoad => self.update_pots(record),
            TelemetryEvent::EpochTransitionCompute => self.rewards_ready = false,
            TelemetryEvent::EpochTransitionRecord => self.epoch_overlay_exists = true,
            TelemetryEvent::EpochTransitionApply => self.epoch_overlay_exists = false,
            TelemetryEvent::StateSwitchToFork => self.push_recent_rollback(record),
            TelemetryEvent::HeaderLifecycle => self.update_peer_header_lifecycle(record),
            TelemetryEvent::KeepaliveRoundTrip => self.update_peer_rtt(record),
            TelemetryEvent::PeerConnected => self.update_peer_connected(record),
            TelemetryEvent::PeerDisconnected => self.update_peer_disconnected(record),
            TelemetryEvent::GovernanceActivityUpdate => {
                self.governance.dormant_epochs =
                    Some(u64::from(ledger::governance_activity::UPDATE::consecutive_dormant_epochs(record)));
            }
            TelemetryEvent::NewGovernanceUpdates => {
                self.governance.proposal_count_in_scope =
                    Some(ledger::epoch_transition::NEW_GOVERNANCE_UPDATES::proposals_count(record));
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
                self.upsert_proposal(record, "skipped", Some(ledger::proposal::SKIP::reason(record).to_owned()))
            }
            TelemetryEvent::ProtocolUpgrade => {
                self.protocol_version = ledger::protocol::UPGRADE::new_version(record).to_string();
            }
            TelemetryEvent::ProtocolParametersLoad | TelemetryEvent::ProtocolParametersRatify => {
                let version = if ledger::protocol_parameters::LOAD::matches(&record.target, &record.name) {
                    ledger::protocol_parameters::LOAD::protocol_version(record)
                } else {
                    ledger::protocol_parameters::RATIFY::protocol_version(record)
                };
                if let Some(version) = version.map(ToOwned::to_owned) {
                    self.protocol_version = version;
                }
            }
            TelemetryEvent::RatificationSummarize => {
                self.governance.latest_ratification = Some(record.to_fields_string());
            }
        }
    }

    fn update_tip(&mut self, record: &TelemetryRecord) {
        let Some(tip) = TipState::from_record(record) else {
            return;
        };

        self.tip = Some(tip);
    }

    fn update_stake_snapshot(&mut self, record: &TelemetryRecord) {
        self.stake_snapshot = StakeSnapshotState::from_record(record);
    }

    fn update_mempool(&mut self, record: &TelemetryRecord) {
        self.mempool = MempoolState {
            tx_count: mempool::state::UPDATE::tx_count(record),
            size_bytes: mempool::state::UPDATE::size_bytes(record),
            updated_at: record.at,
        };
    }

    fn begin_initial_stake_distribution(&mut self, record: &TelemetryRecord) {
        let epoch = ledger::stake_distribution::INITIAL_BEGIN::epoch(record);

        self.initial_stake_distributions_ready = false;
        if !self.initial_stake_distribution_order.contains(&epoch) {
            self.initial_stake_distribution_order.push(epoch);
            self.initial_stake_distribution_order.sort_unstable();
        }

        self.initial_stake_distributions.entry(epoch).or_insert(InitialStakeDistributionState {
            epoch,
            progress: 0.0,
            completed: false,
            updated_at: record.at,
        });
    }

    fn advance_initial_stake_distribution(&mut self, record: &TelemetryRecord) {
        let epoch = ledger::stake_distribution::INITIAL_PROGRESS::epoch(record);
        let progress = ledger::stake_distribution::INITIAL_PROGRESS::progress(record);

        self.begin_initial_stake_distribution(record);
        if let Some(state) = self.initial_stake_distributions.get_mut(&epoch) {
            state.progress = progress.clamp(0.0, 1.0);
            state.updated_at = record.at;
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

    fn push_recent_transaction_count(&mut self, at: Instant, tx_count: u64) {
        let max_window = self.max_window();
        self.recent_transactions.push_back((at, tx_count));

        while self.recent_transactions.front().is_some_and(|(entry_at, _)| at.duration_since(*entry_at) > max_window) {
            self.recent_transactions.pop_front();
        }
    }

    fn record_ledger_metrics(&mut self, at: Instant, metrics: LedgerMetrics) {
        self.push_recent_block(at);
        self.push_recent_transaction_count(at, metrics.tx_count);
    }

    fn record_mempool_metrics(&mut self, at: Instant, metrics: MempoolMetrics) {
        self.mempool = MempoolState { tx_count: metrics.tx_count, size_bytes: metrics.size_bytes, updated_at: at };
    }

    fn record_system_metrics(&mut self, at: Instant, metrics: SystemMetrics) {
        self.push_system_sample(SystemSample {
            at,
            cpu_percent: metrics.cpu_percent,
            process_memory_bytes: metrics.process_memory_bytes,
            rss_bytes: metrics.rss_bytes,
            virtual_bytes: metrics.virtual_bytes,
            memory_used_bytes: metrics.memory_used_bytes,
            memory_total_bytes: metrics.memory_total_bytes,
            disk_read_bytes: metrics.disk_read_bytes,
            disk_write_bytes: metrics.disk_write_bytes,
            disk_live_read_bytes: metrics.disk_live_read_bytes,
            disk_live_write_bytes: metrics.disk_live_write_bytes,
            processes_live_read_bytes: metrics.processes_live_read_bytes,
            processes_live_write_bytes: metrics.processes_live_write_bytes,
        });
    }

    fn push_active_proposal(&mut self, record: &TelemetryRecord) {
        self.upsert_proposal(record, "-", ledger::proposal::ACTIVE::detail(record).map(ToOwned::to_owned));
        self.governance.proposal_count_in_scope = Some(self.governance.proposal_count_in_scope.unwrap_or_default() + 1);
    }

    fn push_dropped_proposal(&mut self, record: &TelemetryRecord) {
        let status = if proposal_id(record)
            .and_then(|id| self.proposals_by_id.get(&id))
            .is_some_and(|proposal| proposal.status == "enacted")
        {
            "enacted"
        } else if ledger::proposal::DROP::expired(record) {
            "expired"
        } else {
            "dropped"
        };

        let detail = if status == "expired" {
            Some("expired".to_string())
        } else if ledger::proposal::DROP::ratified_or_evicted(record) {
            Some("superseded".to_string())
        } else {
            None
        };
        self.upsert_proposal(record, status, detail);
        if let Some(count) = self.governance.proposal_count_in_scope.as_mut() {
            *count = count.saturating_sub(1);
        }
    }

    fn update_pots(&mut self, record: &TelemetryRecord) {
        if ledger::rewards::SUMMARIZE::matches(&record.target, &record.name) {
            self.treasury = Some(ledger::rewards::SUMMARIZE::pots_treasury(record));
            self.reserves = Some(ledger::rewards::SUMMARIZE::pots_reserves(record));
            self.fees = Some(ledger::rewards::SUMMARIZE::pots_fees(record));
            self.donations = self.donations.or(Some(0));
        } else if ledger::pots::LOAD::matches(&record.target, &record.name) {
            self.treasury = Some(ledger::pots::LOAD::treasury(record));
            self.reserves = Some(ledger::pots::LOAD::reserves(record));
            self.fees = Some(ledger::pots::LOAD::fees(record));
            self.donations = Some(ledger::pots::LOAD::donations(record));
        } else if bootstrap::pots::IMPORT::matches(&record.target, &record.name) {
            self.treasury = Some(bootstrap::pots::IMPORT::treasury(record));
            self.reserves = Some(bootstrap::pots::IMPORT::reserves(record));
            self.fees = Some(bootstrap::pots::IMPORT::fees(record));
            self.donations = Some(bootstrap::pots::IMPORT::donations(record));
        }
    }

    fn update_peer_connected(&mut self, record: &TelemetryRecord) {
        let peer = self.peer_mut(protocols::peer_selection::peer::CONNECTED::peer(record), record.at);
        peer.mark_connected(record);
    }

    fn update_peer_disconnected(&mut self, record: &TelemetryRecord) {
        let address = protocols::peer_selection::peer::DISCONNECTED::peer(record);
        if let Some(peer) = self.peers.get_mut(address) {
            peer.mark_disconnected(record);
        }
    }

    fn update_peer_rtt(&mut self, record: &TelemetryRecord) {
        let address = protocols::keepalive::peer::ROUND_TRIP::peer(record);
        let round_trip_micros = protocols::keepalive::peer::ROUND_TRIP::round_trip_micros(record);

        let peer = self.peer_mut(address, record.at);
        peer.connected = true;
        if !peer.inbound && !peer.outbound {
            peer.outbound = true;
        }
        peer.update_rtt(record, round_trip_micros);
    }

    fn update_peer_header_lifecycle(&mut self, record: &TelemetryRecord) {
        let Some(peer) = consensus::perf::header::LIFECYCLE::peer(record) else {
            return;
        };

        let query_header_micros = consensus::perf::header::LIFECYCLE::block_fetch_wait_micros(record);
        let get_block_micros = consensus::perf::header::LIFECYCLE::block_fetch_micros(record);
        let adopt_block_micros = consensus::perf::header::LIFECYCLE::forward_micros(record)
            .zip(query_header_micros)
            .zip(get_block_micros)
            .map(|((forward_micros, query_header_micros), get_block_micros)| {
                forward_micros.saturating_sub(query_header_micros.saturating_add(get_block_micros))
            });

        let peer = self.peer_mut(peer.as_ref(), record.at);
        peer.record_header_lifecycle(record, query_header_micros, get_block_micros, adopt_block_micros);
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

    fn push_recent_rollback(&mut self, record: &TelemetryRecord) {
        let rollback_length = ledger::state::SWITCH_TO_FORK::rollback_length(record);
        let max_window = self.max_window();
        let at = record.at;
        self.recent_rollbacks.push_back((at, rollback_length));

        while self.recent_rollbacks.front().is_some_and(|(entry_at, _)| at.duration_since(*entry_at) > max_window) {
            self.recent_rollbacks.pop_front();
        }
    }

    fn upsert_proposal(&mut self, record: &TelemetryRecord, status: &str, detail: Option<String>) {
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

    fn handle_key_event(&mut self, key: event::KeyEvent) -> TerminalEventOutcome {
        if self.is_copy_mode() {
            return if key.code == KeyCode::Esc {
                self.exit_copy_mode();
                TerminalEventOutcome::ExitCopyMode
            } else {
                TerminalEventOutcome::Continue
            };
        }

        match key.code {
            KeyCode::Esc => {
                self.enter_copy_mode();
                TerminalEventOutcome::EnterCopyMode
            }
            KeyCode::Char('q') => TerminalEventOutcome::Shutdown,
            KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                TerminalEventOutcome::Shutdown
            }
            KeyCode::Tab => {
                self.next_page();
                TerminalEventOutcome::Continue
            }
            KeyCode::BackTab => {
                self.previous_page();
                TerminalEventOutcome::Continue
            }
            KeyCode::Right => {
                self.next_scroll_focus();
                TerminalEventOutcome::Continue
            }
            KeyCode::Left => {
                self.previous_scroll_focus();
                TerminalEventOutcome::Continue
            }
            KeyCode::Enter => {
                let _ = self.toggle_focused_pane();
                TerminalEventOutcome::Continue
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let _ = self.toggle_focused_pane();
                TerminalEventOutcome::Continue
            }
            KeyCode::Up => {
                self.scroll_focused(-1);
                TerminalEventOutcome::Continue
            }
            KeyCode::Down => {
                self.scroll_focused(1);
                TerminalEventOutcome::Continue
            }
            KeyCode::PageUp => {
                self.scroll_focused(-10);
                TerminalEventOutcome::Continue
            }
            KeyCode::PageDown => {
                self.scroll_focused(10);
                TerminalEventOutcome::Continue
            }
            KeyCode::Backspace
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::F(_)
            | KeyCode::Char(_)
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => TerminalEventOutcome::Continue,
        }
    }

    fn handle_mouse_event(&mut self, mouse: event::MouseEvent, views: &Views) -> TerminalEventOutcome {
        let point = Rect { x: mouse.column, y: mouse.row, width: 1, height: 1 };

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(views, point),
            MouseEventKind::ScrollDown => self.handle_scroll(views, point, 3),
            MouseEventKind::ScrollUp => self.handle_scroll(views, point, -3),
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {}
        }

        TerminalEventOutcome::Continue
    }

    fn set_scroll_focus(&mut self, focus: ScrollFocus) {
        self.scroll_focus = focus;
    }
}

fn prune_recent(entries: &mut VecDeque<Instant>, now: Instant, max_window: Duration) {
    while entries.front().is_some_and(|at| now.duration_since(*at) > max_window) {
        entries.pop_front();
    }
}

pub fn render_fields(record: &TelemetryRecord) -> String {
    record.to_fields_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use amaru_metrics::{MetricsEvent, ledger::LedgerMetrics, system::SystemMetrics};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracing::Level;

    use super::*;
    use crate::{
        events::{FieldValue, TelemetryKind, TelemetryRecord},
        model::telemetry_event::{LEDGER_TARGET, PROTOCOLS_TARGET},
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

    fn metric(at: Instant, event: MetricsEvent) -> Message {
        Message::Metrics(crate::events::MetricRecord { at, event })
    }

    fn startup_context() -> StartupContext {
        StartupContext {
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
    fn records_throughput_and_system_samples_from_metrics() {
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
        let at = Instant::now();

        model.handle_message(metric(
            at,
            MetricsEvent::LedgerMetrics(LedgerMetrics {
                block_height: 42,
                tx_count: 7,
                slot: 100,
                slot_in_epoch: 10,
                epoch: 1,
                density: 0.5,
                current_kes_period: 2,
                remaining_kes_periods: 3,
                block_header_hash: "abc".into(),
                parent_block_header_hash: "def".into(),
                issuer_verification_key_hash: "ghi".into(),
            }),
        ));
        model.handle_message(metric(
            at,
            MetricsEvent::SystemMetrics(SystemMetrics {
                runtime_seconds: 1,
                cpu_percent: 12.5,
                process_memory_bytes: 10_000,
                rss_bytes: 9_000,
                virtual_bytes: 12_000,
                memory_used_bytes: 100_000,
                memory_total_bytes: 200_000,
                disk_read_bytes: 300,
                disk_write_bytes: 400,
                disk_live_read_bytes: 30,
                disk_live_write_bytes: 40,
                processes_live_read_bytes: 300,
                processes_live_write_bytes: 400,
                open_files: 5,
            }),
        ));

        assert_eq!(model.blocks_in_window(at + Duration::from_secs(1)), 1);
        assert_eq!(model.transactions_in_window(at + Duration::from_secs(1)), 7);
        assert_eq!(model.system_samples.back().map(|sample| sample.process_memory_bytes), Some(10_000));
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
    fn tracks_peer_header_lifecycle_means() {
        let mut model = Model::new(Config::default(), startup_context());

        model.handle_message(Message::Telemetry(telemetry(
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("block_fetch_wait_micros", FieldValue::U64(2_000)),
                ("block_fetch_micros", FieldValue::U64(5_000)),
                ("forward_micros", FieldValue::U64(11_000)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry(
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("block_fetch_wait_micros", FieldValue::U64(4_000)),
                ("block_fetch_micros", FieldValue::U64(7_000)),
                ("forward_micros", FieldValue::U64(15_000)),
            ],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_query_header_micros(), Some(3_000));
        assert_eq!(peer.mean_get_block_micros(), Some(6_000));
        assert_eq!(peer.mean_adopt_block_micros(), Some(4_000));
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

    #[test]
    fn keyboard_navigation_uses_arrows_for_focus_and_enter_for_pane_toggle() {
        let mut model = Model::new(Config::default(), startup_context());

        assert_eq!(model.page, Page::Amaru);
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);
        assert_eq!(model.log_pane_mode, PaneMode::Normal);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.page, Page::Amaru);
        assert_eq!(model.scroll_focus, ScrollFocus::Peers);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.peer_pane_mode, PaneMode::Maximized);
        assert_eq!(model.log_pane_mode, PaneMode::Normal);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.page, Page::Cardano);
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.scroll_focus, ScrollFocus::Proposals);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.proposal_pane_mode, PaneMode::Maximized);
    }

    #[test]
    fn config_page_uses_its_own_scroll_focus() {
        let mut model = Model::new(Config::default(), startup_context());

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );

        assert_eq!(model.page, Page::Config);
        assert_eq!(model.scroll_focus, ScrollFocus::Config);
        assert_eq!(model.config_scroll, 0);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.config_scroll, 1);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.scroll_focus, ScrollFocus::Config);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.page, Page::Cardano);
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);
    }
}
