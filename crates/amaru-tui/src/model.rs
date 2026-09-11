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
    rc::Rc,
    time::Instant,
};

use self::{
    governance_summary::GovernanceSummary,
    log_buffer::LogBuffer,
    mempool_state::MempoolState,
    proposal_activity::{ProposalActivity, proposal_id},
    rate_counter::RateCounter,
    stake_snapshot_state::StakeSnapshotState,
    telemetry_event::TelemetryEvent,
    tip_state::TipState,
};
use crate::{
    config::Config,
    events::{SystemSample, TelemetryRecord},
    startup::StartupContext,
};

mod command_menu;
mod exponential_moving_average;
mod governance_summary;
mod initial_stake_distribution_state;
mod interaction;
mod interaction_mode;
mod level_filter;
mod log_buffer;
mod log_time;
mod mempool_state;
mod metrics_update;
mod page;
mod pane_mode;
mod peer_state;
mod prompt;
mod proposal_activity;
mod queries;
mod rate_counter;
mod scroll_focus;
pub(crate) mod scrollbar;
mod stake_snapshot_state;
mod target_filter;
mod telemetry_event;
mod telemetry_update;
mod terminal_event_outcome;
mod tip_state;

pub(crate) use self::command_menu::CommandMenu;
pub use self::{
    initial_stake_distribution_state::InitialStakeDistributionState,
    interaction_mode::InteractionMode,
    level_filter::LevelFilter,
    log_buffer::{LogViewItem, RetentionTier},
    page::Page,
    pane_mode::PaneMode,
    peer_state::PeerState,
    prompt::{PromptKind, PromptState},
    scroll_focus::ScrollFocus,
    target_filter::TargetFilter,
    terminal_event_outcome::TerminalEventOutcome,
};

#[derive(Debug)]
pub struct Model {
    pub startup: StartupContext,
    pub page: Page,
    pub interaction_mode: InteractionMode,
    pub(crate) command_menu: CommandMenu,
    pub log_pane_mode: PaneMode,
    pub peer_pane_mode: PaneMode,
    pub proposal_pane_mode: PaneMode,
    pub scroll_focus: ScrollFocus,
    pub level_filter: LevelFilter,
    pub target_filter: TargetFilter,
    pub text_filter_pattern: String,
    pub highlight_pattern: String,
    pub prompt: Option<PromptState>,
    pub catching_up: bool,
    pub log_scroll: usize,
    pub log_hscroll: usize,
    pub log_wrap: bool,
    pub log_scrollbar_focused: bool,
    log_scrollbar_drag: bool,
    pub peer_scroll: usize,
    pub proposal_scroll: usize,
    pub config_scroll: usize,
    pub created_at: Instant,
    pub tip: Option<TipState>,
    pub tip_sync_origin: Option<(u64, Instant)>,
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
    /// `peer.resolved` cache: dial address → bootstrap name (omitted when the candidate was already a Peer).
    resolved_candidates: BTreeMap<String, String>,
    pub logs: LogBuffer,
    text_filter: Option<regex::Regex>,
    highlight: Option<regex::Regex>,
    log_cursor: Option<Rc<TelemetryRecord>>,
    logs_viewport_rows: usize,
    logs_viewport_columns: usize,
    peers_viewport_rows: usize,
    proposals_viewport_rows: usize,
    config_viewport_rows: usize,
    pub system_sample: Option<SystemSample>,
    pub block_rate: RateCounter,
    pub transaction_rate: RateCounter,
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
            command_menu: CommandMenu::Default,
            log_pane_mode: PaneMode::Normal,
            peer_pane_mode: PaneMode::Normal,
            proposal_pane_mode: PaneMode::Normal,
            scroll_focus: ScrollFocus::Logs,
            level_filter: LevelFilter::Info,
            target_filter: TargetFilter::All,
            text_filter_pattern: String::new(),
            highlight_pattern: String::new(),
            prompt: None,
            catching_up: true,
            log_scroll: 0,
            log_hscroll: 0,
            log_wrap: true,
            log_scrollbar_focused: false,
            log_scrollbar_drag: false,
            peer_scroll: 0,
            proposal_scroll: 0,
            config_scroll: 0,
            created_at: Instant::now(),
            tip: None,
            tip_sync_origin: None,
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
            resolved_candidates: BTreeMap::default(),
            logs: LogBuffer::new(config.log_retention_bytes),
            text_filter: None,
            highlight: None,
            log_cursor: None,
            logs_viewport_rows: 10,
            logs_viewport_columns: 80,
            peers_viewport_rows: 10,
            proposals_viewport_rows: 10,
            config_viewport_rows: 10,
            system_sample: None,
            block_rate: RateCounter::new(config.block_sample_capacity),
            transaction_rate: RateCounter::new(config.transaction_sample_capacity),
            recent_rollbacks: VecDeque::default(),
            initial_stake_distribution_order: Vec::default(),
            initial_stake_distributions: BTreeMap::default(),
            initial_stake_distributions_ready: false,
            proposal_order: VecDeque::default(),
            proposals_by_id: BTreeMap::default(),
            config,
        }
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
}

pub fn render_fields(record: &TelemetryRecord) -> String {
    record.to_fields_string()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use amaru_metrics::{MetricsEvent, system::SystemMetrics};
    use amaru_observability::amaru::{consensus, ledger, protocols};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use tracing::Level;

    use super::*;
    use crate::{
        events::{FieldValue, Message, TelemetryRecord},
        startup::ProcessInfo,
        ui::Views,
    };

    fn telemetry_record<const N: usize>(
        at: Instant,
        target: &str,
        name: &str,
        fields: [(&str, FieldValue); N],
    ) -> TelemetryRecord {
        TelemetryRecord {
            level: Level::INFO,
            target: target.into(),
            name: name.into(),
            at,
            wall_time: std::time::SystemTime::UNIX_EPOCH,
            fields: fields.into_iter().map(|(name, value)| (name.into(), value)).collect(),
            parents: Vec::new(),
            span_name: None,
            id: None,
            parent_id: None,
        }
    }

    macro_rules! telemetry {
        ($schema:path $(, $field:path => $value:expr )* $(,)?) => {
            telemetry_at!(Instant::now(), $schema $(, $field => $value )*)
        };
    }

    macro_rules! telemetry_at {
        ($at:expr, $schema:path $(, $field:path => $value:expr )* $(,)?) => {
            telemetry_record(
                $at,
                <$schema>::TARGET,
                <$schema>::NAME,
                [$(($field, FieldValue::from($value))),*],
            )
        };
    }

    fn metric(at: Instant, event: MetricsEvent) -> Message {
        Message::Metrics(crate::events::MetricRecord { at, event })
    }

    fn fixture_startup_context() -> StartupContext {
        StartupContext {
            process: ProcessInfo {
                pid: 42,
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            consensus_security_param: 432,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            runtime_sections: Vec::default(),
            protocol_sections: Vec::default(),
        }
    }

    #[test]
    fn updates_tip_from_public_event() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::tip::UPDATE,
            ledger::tip::UPDATE::FIELD_SLOT => 1u64,
            ledger::tip::UPDATE::FIELD_HEADER_HASH => "abc",
            ledger::tip::UPDATE::FIELD_BLOCK_HEIGHT => 2u64,
            ledger::tip::UPDATE::FIELD_TX_COUNT => 7u64,
            ledger::tip::UPDATE::FIELD_EPOCH => 3u64,
            ledger::tip::UPDATE::FIELD_SLOT_IN_EPOCH => 4u64,
            ledger::tip::UPDATE::FIELD_DENSITY => 0.5f64,
            ledger::tip::UPDATE::FIELD_CURRENT_KES_PERIOD => 5u64,
            ledger::tip::UPDATE::FIELD_REMAINING_KES_PERIODS => 6u64,
        )));

        let tip = model.tip.expect("tip must be recorded");
        assert_eq!(tip.slot, 1);
        assert_eq!(tip.epoch, 3);
        assert_eq!(tip.block_height, 2);
    }

    #[test]
    fn records_process_memory_from_system_metrics() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        let at = Instant::now();

        model.handle_message(metric(
            at,
            MetricsEvent::SystemMetrics(SystemMetrics {
                runtime_seconds: 1,
                cpu_percent: 12.5,
                process_memory_bytes: 15_000,
                process_memory_live_resident: 9_000,
                process_memory_available_virtual: 12_000,
                memory_used_bytes: 100_000,
                memory_total_bytes: 200_000,
                disk_read_bytes: 300,
                disk_write_bytes: 400,
                disk_live_read_bytes: 30,
                disk_live_write_bytes: 40,
                host_live_read_bytes: 300,
                host_live_write_bytes: 500,
                open_files: 5,
            }),
        ));
        model.handle_message(metric(
            at + Duration::from_secs(6),
            MetricsEvent::SystemMetrics(SystemMetrics {
                runtime_seconds: 2,
                cpu_percent: 14.5,
                process_memory_bytes: 16_000,
                process_memory_live_resident: 9_500,
                process_memory_available_virtual: 12_500,
                memory_used_bytes: 110_000,
                memory_total_bytes: 210_000,
                disk_read_bytes: 350,
                disk_write_bytes: 450,
                disk_live_read_bytes: 35,
                disk_live_write_bytes: 45,
                host_live_read_bytes: 350,
                host_live_write_bytes: 550,
                open_files: 6,
            }),
        ));

        assert_eq!(model.system_sample.as_ref().map(|sample| sample.process_memory_bytes), Some(16_000));
        assert_eq!(model.system_sample.as_ref().map(|sample| sample.rss_bytes), Some(9_500));
    }

    #[test]
    fn records_throughput_from_roll_forwards_transaction_validations_and_system_samples_from_metrics() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        let at = model.created_at + Duration::from_secs(1);

        model.handle_message(metric(
            at,
            MetricsEvent::SystemMetrics(SystemMetrics {
                runtime_seconds: 1,
                cpu_percent: 12.5,
                process_memory_bytes: 18_000,
                process_memory_live_resident: 9_000,
                process_memory_available_virtual: 12_000,
                memory_used_bytes: 100_000,
                memory_total_bytes: 200_000,
                disk_read_bytes: 300,
                disk_write_bytes: 400,
                disk_live_read_bytes: 30,
                disk_live_write_bytes: 40,
                host_live_read_bytes: 300,
                host_live_write_bytes: 500,
                open_files: 5,
            }),
        ));
        model.handle_message(Message::Telemetry(telemetry_at!(
            at + Duration::from_millis(500),
            ledger::state::ROLL_FORWARD,
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            at + Duration::from_millis(500),
            ledger::tip::UPDATE,
            ledger::tip::UPDATE::FIELD_SLOT => 100u64,
            ledger::tip::UPDATE::FIELD_HEADER_HASH => "abc",
            ledger::tip::UPDATE::FIELD_BLOCK_HEIGHT => 42u64,
            ledger::tip::UPDATE::FIELD_TX_COUNT => 7u64,
            ledger::tip::UPDATE::FIELD_EPOCH => 1u64,
            ledger::tip::UPDATE::FIELD_SLOT_IN_EPOCH => 10u64,
            ledger::tip::UPDATE::FIELD_DENSITY => 0.5f64,
            ledger::tip::UPDATE::FIELD_CURRENT_KES_PERIOD => 2u64,
            ledger::tip::UPDATE::FIELD_REMAINING_KES_PERIODS => 3u64,
        )));
        for index in 0..7 {
            model.handle_message(Message::Telemetry(telemetry_at!(
                at + Duration::from_millis(500 + index),
                ledger::transaction::VALIDATE,
                ledger::transaction::VALIDATE::FIELD_ID => format!("tx-{index}"),
            )));
        }
        model.handle_message(metric(
            at + Duration::from_secs(1),
            MetricsEvent::SystemMetrics(SystemMetrics {
                runtime_seconds: 2,
                cpu_percent: 12.5,
                process_memory_bytes: 18_000,
                process_memory_live_resident: 9_000,
                process_memory_available_virtual: 12_000,
                memory_used_bytes: 100_000,
                memory_total_bytes: 200_000,
                disk_read_bytes: 300,
                disk_write_bytes: 400,
                disk_live_read_bytes: 30,
                disk_live_write_bytes: 40,
                host_live_read_bytes: 300,
                host_live_write_bytes: 500,
                open_files: 5,
            }),
        ));

        assert_eq!(model.recent_blocks_count(), 1);
        assert_eq!(model.recent_transactions_count(), 7);
        assert_eq!(model.blocks_per_second(), 1.0);
        assert_eq!(model.transactions_per_second(), 7.0);
        assert_eq!(model.system_sample.as_ref().map(|sample| sample.process_memory_bytes), Some(18_000));
        assert_eq!(model.system_sample.as_ref().map(|sample| sample.memory_total_bytes), Some(200_000));
        assert_eq!(model.system_sample.as_ref().map(|sample| sample.host_live_read_bytes), Some(300));
    }

    #[test]
    fn updates_peer_rtt() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            protocols::keepalive::peer::ROUND_TRIP,
            protocols::keepalive::peer::ROUND_TRIP::FIELD_PEER => "1.2.3.4:3001",
            protocols::keepalive::peer::ROUND_TRIP::FIELD_CONN_ID => "7",
            protocols::keepalive::peer::ROUND_TRIP::FIELD_ROUND_TRIP_MICROS => 12_345u64,
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.last_rtt_micros, Some(12_345));
    }

    #[test]
    fn caches_resolved_candidate_for_later_peer_row() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            protocols::peer_selection::peer::RESOLVED,
            protocols::peer_selection::peer::RESOLVED::FIELD_CANDIDATE => "relay.example:3001",
            protocols::peer_selection::peer::RESOLVED::FIELD_ORIGIN => "static",
            protocols::peer_selection::peer::RESOLVED::FIELD_PEER => "10.9.9.9:3001",
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            protocols::peer_selection::peer::CONNECTED,
            protocols::peer_selection::peer::CONNECTED::FIELD_PEER => "10.9.9.9:3001",
            protocols::peer_selection::peer::CONNECTED::FIELD_CONN_ID => 1u64,
            protocols::peer_selection::peer::CONNECTED::FIELD_DIRECTION => "Outbound",
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX_CAPABLE => true,
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX => false,
        )));

        let peer = model.peers.get("10.9.9.9:3001").expect("peer must exist");
        assert_eq!(peer.candidate_label(), Some("relay.example:3001"));
    }

    #[test]
    fn resolved_socket_candidate_is_not_shown() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            protocols::peer_selection::peer::RESOLVED,
            protocols::peer_selection::peer::RESOLVED::FIELD_CANDIDATE => "10.9.9.9:3001",
            protocols::peer_selection::peer::RESOLVED::FIELD_ORIGIN => "static",
            protocols::peer_selection::peer::RESOLVED::FIELD_PEER => "10.9.9.9:3001",
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            protocols::keepalive::peer::ROUND_TRIP,
            protocols::keepalive::peer::ROUND_TRIP::FIELD_PEER => "10.9.9.9:3001",
            protocols::keepalive::peer::ROUND_TRIP::FIELD_ROUND_TRIP_MICROS => 1_000u64,
        )));

        let peer = model.peers.get("10.9.9.9:3001").expect("peer must exist");
        assert_eq!(peer.candidate, None);
        assert_eq!(peer.candidate_label(), None);
    }

    #[test]
    fn resolved_candidate_updates_an_existing_peer_row() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            protocols::peer_selection::peer::CONNECTED,
            protocols::peer_selection::peer::CONNECTED::FIELD_PEER => "10.8.8.8:6000",
            protocols::peer_selection::peer::CONNECTED::FIELD_CONN_ID => 1u64,
            protocols::peer_selection::peer::CONNECTED::FIELD_DIRECTION => "Outbound",
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX_CAPABLE => true,
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX => false,
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            protocols::peer_selection::peer::RESOLVED,
            protocols::peer_selection::peer::RESOLVED::FIELD_CANDIDATE => "pool.example",
            protocols::peer_selection::peer::RESOLVED::FIELD_ORIGIN => "snapshot",
            protocols::peer_selection::peer::RESOLVED::FIELD_PEER => "10.8.8.8:6000",
        )));

        let peer = model.peers.get("10.8.8.8:6000").expect("peer must exist");
        assert_eq!(peer.candidate_label(), Some("pool.example"));
    }

    #[test]
    fn resolved_cache_survives_peer_prune() {
        let mut model = Model::new(
            Config { peer_inactivity_timeout: Duration::from_secs(30), ..Config::default() },
            fixture_startup_context(),
        );
        let first = Instant::now();
        let later = first + Duration::from_secs(31);

        model.handle_message(Message::Telemetry(telemetry_at!(
            first,
            protocols::peer_selection::peer::RESOLVED,
            protocols::peer_selection::peer::RESOLVED::FIELD_CANDIDATE => "relay.example:3001",
            protocols::peer_selection::peer::RESOLVED::FIELD_ORIGIN => "static",
            protocols::peer_selection::peer::RESOLVED::FIELD_PEER => "10.9.9.9:3001",
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            first,
            protocols::peer_selection::peer::CONNECTED,
            protocols::peer_selection::peer::CONNECTED::FIELD_PEER => "10.9.9.9:3001",
            protocols::peer_selection::peer::CONNECTED::FIELD_CONN_ID => 1u64,
            protocols::peer_selection::peer::CONNECTED::FIELD_DIRECTION => "Outbound",
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX_CAPABLE => true,
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX => false,
        )));
        model.handle_message(metric(
            later,
            MetricsEvent::SystemMetrics(SystemMetrics { process_memory_bytes: 1, ..SystemMetrics::default() }),
        ));
        assert!(!model.peers.contains_key("10.9.9.9:3001"));

        model.handle_message(Message::Telemetry(telemetry_at!(
            later,
            protocols::peer_selection::peer::CONNECTED,
            protocols::peer_selection::peer::CONNECTED::FIELD_PEER => "10.9.9.9:3001",
            protocols::peer_selection::peer::CONNECTED::FIELD_CONN_ID => 2u64,
            protocols::peer_selection::peer::CONNECTED::FIELD_DIRECTION => "Outbound",
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX_CAPABLE => true,
            protocols::peer_selection::peer::CONNECTED::FIELD_FULL_DUPLEX => false,
        )));

        let peer = model.peers.get("10.9.9.9:3001").expect("peer must exist");
        assert_eq!(peer.candidate_label(), Some("relay.example:3001"));
    }

    #[test]
    fn sorts_peers_by_ascending_rtt() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        let now = Instant::now();

        let mut fast = PeerState::new("fast.example:3001".into(), now);
        fast.last_rtt_micros = Some(5_000);

        let mut unknown = PeerState::new("unknown.example:3001".into(), now);
        unknown.last_rtt_micros = None;

        let mut slow = PeerState::new("slow.example:3001".into(), now);
        slow.last_rtt_micros = Some(15_000);

        model.peers.insert(fast.address.clone(), fast);
        model.peers.insert(unknown.address.clone(), unknown);
        model.peers.insert(slow.address.clone(), slow);

        let peers = model.sorted_peers();
        let addresses = peers.iter().map(|peer| peer.address.as_str()).collect::<Vec<_>>();

        assert_eq!(addresses, vec!["fast.example:3001", "slow.example:3001", "unknown.example:3001"]);
    }

    #[test]
    fn prunes_stale_peers_after_the_inactivity_timeout() {
        let mut model = Model::new(
            Config { peer_inactivity_timeout: Duration::from_secs(30), ..Config::default() },
            fixture_startup_context(),
        );
        let stale_at = Instant::now();
        let now = stale_at + Duration::from_secs(31);

        model.peers.insert("stale.example:3001".into(), PeerState::new("stale.example:3001".into(), stale_at));
        model.peers.insert(
            "recent.example:3001".into(),
            PeerState::new("recent.example:3001".into(), now - Duration::from_secs(1)),
        );

        model.handle_message(metric(
            now,
            MetricsEvent::SystemMetrics(SystemMetrics { process_memory_bytes: 1, ..SystemMetrics::default() }),
        ));

        assert!(!model.peers.contains_key("stale.example:3001"));
        assert!(model.peers.contains_key("recent.example:3001"));
    }

    #[test]
    fn tracks_peer_header_lifecycle_emas() {
        let mut model = Model::new(Config { peer_timing_capacity: 2, ..Config::default() }, fixture_startup_context());
        let now = model.created_at;

        model.handle_message(Message::Telemetry(telemetry_at!(
            now,
            consensus::tip::ADOPT,
            consensus::tip::ADOPT::FIELD_SLOT => 1u64,
            consensus::tip::ADOPT::FIELD_HEADER_HASH => "abc",
            consensus::tip::ADOPT::FIELD_BLOCK_HEIGHT => 10u64,
            consensus::tip::ADOPT::FIELD_MAX_BLOCK_HEIGHT => 10u64,
            consensus::tip::ADOPT::FIELD_SUPPRESSED => 0u32,
        )));

        model.handle_message(Message::Telemetry(telemetry_at!(
            now,
            consensus::perf::header::LIFECYCLE,
            consensus::perf::header::LIFECYCLE::FIELD_PEER => "1.2.3.4:3001",
            consensus::perf::header::LIFECYCLE::FIELD_OUTCOME => "valid",
            consensus::perf::header::LIFECYCLE::FIELD_SLOT_START_TO_HEADER_MICROS => 9_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_WAIT_MICROS => 2_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_MICROS => 5_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_FORWARD_MICROS => 11_000u64,
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            now + Duration::from_secs(1),
            consensus::perf::header::LIFECYCLE,
            consensus::perf::header::LIFECYCLE::FIELD_PEER => "1.2.3.4:3001",
            consensus::perf::header::LIFECYCLE::FIELD_OUTCOME => "valid",
            consensus::perf::header::LIFECYCLE::FIELD_SLOT_START_TO_HEADER_MICROS => 15_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_WAIT_MICROS => 4_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_MICROS => 7_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_FORWARD_MICROS => 15_000u64,
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_slot_start_to_header_micros(), Some(13_000));
        assert_eq!(peer.mean_query_header_micros(), Some(3_333));
        assert_eq!(peer.mean_get_block_micros(), Some(6_333));
        assert_eq!(peer.mean_adopt_block_micros(), Some(4_000));
    }

    #[test]
    fn peer_header_lifecycle_emas_follow_the_latest_sample_when_smoothing_is_one() {
        let config = Config { peer_timing_capacity: 1, ..Config::default() };
        let mut model = Model::new(config, fixture_startup_context());
        let now = model.created_at;

        model.handle_message(Message::Telemetry(telemetry_at!(
            now,
            consensus::tip::ADOPT,
            consensus::tip::ADOPT::FIELD_SLOT => 1u64,
            consensus::tip::ADOPT::FIELD_HEADER_HASH => "abc",
            consensus::tip::ADOPT::FIELD_BLOCK_HEIGHT => 10u64,
            consensus::tip::ADOPT::FIELD_MAX_BLOCK_HEIGHT => 10u64,
            consensus::tip::ADOPT::FIELD_SUPPRESSED => 0u32,
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            now,
            consensus::perf::header::LIFECYCLE,
            consensus::perf::header::LIFECYCLE::FIELD_PEER => "1.2.3.4:3001",
            consensus::perf::header::LIFECYCLE::FIELD_OUTCOME => "valid",
            consensus::perf::header::LIFECYCLE::FIELD_SLOT_START_TO_HEADER_MICROS => 9_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_WAIT_MICROS => 2_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_MICROS => 5_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_FORWARD_MICROS => 11_000u64,
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            now + Duration::from_secs(10),
            consensus::perf::header::LIFECYCLE,
            consensus::perf::header::LIFECYCLE::FIELD_PEER => "1.2.3.4:3001",
            consensus::perf::header::LIFECYCLE::FIELD_OUTCOME => "valid",
            consensus::perf::header::LIFECYCLE::FIELD_SLOT_START_TO_HEADER_MICROS => 15_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_WAIT_MICROS => 4_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_MICROS => 7_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_FORWARD_MICROS => 15_000u64,
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_query_header_micros(), Some(4_000));
        assert_eq!(peer.mean_slot_start_to_header_micros(), Some(15_000));
    }

    #[test]
    fn slot_start_timing_is_hidden_while_catching_up() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        let now = model.created_at;

        model.handle_message(Message::Telemetry(telemetry_at!(
            now,
            consensus::perf::header::LIFECYCLE,
            consensus::perf::header::LIFECYCLE::FIELD_PEER => "1.2.3.4:3001",
            consensus::perf::header::LIFECYCLE::FIELD_OUTCOME => "valid",
            consensus::perf::header::LIFECYCLE::FIELD_SLOT_START_TO_HEADER_MICROS => 9_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_WAIT_MICROS => 2_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_MICROS => 5_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_FORWARD_MICROS => 11_000u64,
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_slot_start_to_header_micros(), None);

        model.handle_message(Message::Telemetry(telemetry_at!(
            now + Duration::from_secs(1),
            consensus::tip::ADOPT,
            consensus::tip::ADOPT::FIELD_SLOT => 1u64,
            consensus::tip::ADOPT::FIELD_HEADER_HASH => "abc",
            consensus::tip::ADOPT::FIELD_BLOCK_HEIGHT => 10u64,
            consensus::tip::ADOPT::FIELD_MAX_BLOCK_HEIGHT => 100u64,
            consensus::tip::ADOPT::FIELD_SUPPRESSED => 0u32,
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            now + Duration::from_secs(2),
            consensus::tip::ADOPT,
            consensus::tip::ADOPT::FIELD_SLOT => 1u64,
            consensus::tip::ADOPT::FIELD_HEADER_HASH => "abc",
            consensus::tip::ADOPT::FIELD_BLOCK_HEIGHT => 100u64,
            consensus::tip::ADOPT::FIELD_MAX_BLOCK_HEIGHT => 100u64,
            consensus::tip::ADOPT::FIELD_SUPPRESSED => 0u32,
        )));
        model.handle_message(Message::Telemetry(telemetry_at!(
            now + Duration::from_secs(3),
            consensus::perf::header::LIFECYCLE,
            consensus::perf::header::LIFECYCLE::FIELD_PEER => "1.2.3.4:3001",
            consensus::perf::header::LIFECYCLE::FIELD_OUTCOME => "valid",
            consensus::perf::header::LIFECYCLE::FIELD_SLOT_START_TO_HEADER_MICROS => 3_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_WAIT_MICROS => 1_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_BLOCK_FETCH_MICROS => 2_000u64,
            consensus::perf::header::LIFECYCLE::FIELD_FORWARD_MICROS => 5_000u64,
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_slot_start_to_header_micros(), Some(3_000));
    }

    #[test]
    fn waits_for_initial_stake_distributions_ready_event() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        let later = model.created_at + Duration::from_secs(60);

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::stake_distribution::INITIAL_BEGIN,
            ledger::stake_distribution::INITIAL_BEGIN::FIELD_EPOCH => 100u64,
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::stake_distribution::INITIAL_PROGRESS,
            ledger::stake_distribution::INITIAL_PROGRESS::FIELD_EPOCH => 100u64,
            ledger::stake_distribution::INITIAL_PROGRESS::FIELD_PROGRESS => 0.42f64,
        )));

        assert!(!model.is_ready(later));
        assert_eq!(model.initial_stake_distributions().count(), 1);
        assert_eq!(model.initial_stake_distributions().next().map(|state| state.progress), Some(0.42));

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::stake_distribution::INITIAL_READY,
            ledger::stake_distribution::INITIAL_READY::FIELD_EPOCHS => "100",
        )));

        assert!(model.is_ready(later));
        assert_eq!(model.initial_stake_distributions().next().map(|state| state.progress), Some(1.0));
        assert_eq!(model.initial_stake_distributions().next().map(|state| state.completed), Some(true));
    }

    #[test]
    fn initial_stake_distributions_are_ordered_by_epoch() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::stake_distribution::INITIAL_BEGIN,
            ledger::stake_distribution::INITIAL_BEGIN::FIELD_EPOCH => 101u64,
        )));

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::stake_distribution::INITIAL_BEGIN,
            ledger::stake_distribution::INITIAL_BEGIN::FIELD_EPOCH => 99u64,
        )));

        let epochs = model.initial_stake_distributions().map(|state| state.epoch).collect::<Vec<_>>();

        assert_eq!(epochs, vec![99, 101]);
    }

    #[test]
    fn stake_distribution_begin_closes_an_open_prompt() {
        let mut model = ready_model();
        model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(model.prompt_is_open());

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::stake_distribution::INITIAL_BEGIN,
            ledger::stake_distribution::INITIAL_BEGIN::FIELD_EPOCH => 100u64,
        )));

        assert!(!model.prompt_is_open());
        assert!(!model.is_ready(Instant::now()));
    }

    #[test]
    fn proposal_drop_distinguishes_expired_dropped_and_enacted() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::governance::ENACTING,
            ledger::governance::ENACTING::FIELD_PROPOSAL_ID => "enacted",
            ledger::governance::ENACTING::FIELD_PROPOSAL_KIND => "constitution",
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::proposal::DROP,
            ledger::proposal::DROP::FIELD_ID => "enacted",
            ledger::proposal::DROP::FIELD_EXPIRED => false,
            ledger::proposal::DROP::FIELD_RATIFIED_OR_EVICTED => true,
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::proposal::ACTIVE,
            ledger::proposal::ACTIVE::FIELD_ID => "expired",
            ledger::proposal::ACTIVE::FIELD_PROPOSAL_KIND => "hard-fork",
            ledger::proposal::ACTIVE::FIELD_PROPOSED_IN => 10u64,
            ledger::proposal::ACTIVE::FIELD_VALID_UNTIL => 12u64,
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::proposal::DROP,
            ledger::proposal::DROP::FIELD_ID => "expired",
            ledger::proposal::DROP::FIELD_EXPIRED => true,
            ledger::proposal::DROP::FIELD_RATIFIED_OR_EVICTED => false,
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::proposal::ACTIVE,
            ledger::proposal::ACTIVE::FIELD_ID => "dropped",
            ledger::proposal::ACTIVE::FIELD_PROPOSAL_KIND => "treasury-withdrawal",
            ledger::proposal::ACTIVE::FIELD_PROPOSED_IN => 10u64,
            ledger::proposal::ACTIVE::FIELD_VALID_UNTIL => 12u64,
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::proposal::DROP,
            ledger::proposal::DROP::FIELD_ID => "dropped",
            ledger::proposal::DROP::FIELD_EXPIRED => false,
            ledger::proposal::DROP::FIELD_RATIFIED_OR_EVICTED => true,
        )));

        assert_eq!(model.proposals_by_id.get("enacted").map(|proposal| proposal.status.as_str()), Some("enacted"));
        assert_eq!(model.proposals_by_id.get("expired").map(|proposal| proposal.status.as_str()), Some("expired"));
        assert_eq!(model.proposals_by_id.get("dropped").map(|proposal| proposal.status.as_str()), Some("dropped"));
    }

    #[test]
    fn proposal_drop_keeps_enacted_status_even_when_expired_flag_is_set() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

        model.handle_message(Message::Telemetry(telemetry!(
            ledger::governance::ENACTING,
            ledger::governance::ENACTING::FIELD_PROPOSAL_ID => "proposal",
            ledger::governance::ENACTING::FIELD_PROPOSAL_KIND => "protocol-parameters",
        )));
        model.handle_message(Message::Telemetry(telemetry!(
            ledger::proposal::DROP,
            ledger::proposal::DROP::FIELD_ID => "proposal",
            ledger::proposal::DROP::FIELD_EXPIRED => true,
            ledger::proposal::DROP::FIELD_RATIFIED_OR_EVICTED => true,
        )));

        assert_eq!(model.proposals_by_id.get("proposal").map(|proposal| proposal.status.as_str()), Some("enacted"));
    }

    #[test]
    fn keepalive_rtt_marks_peer_as_outbound_when_direction_is_missing() {
        let startup = StartupContext {
            process: ProcessInfo {
                pid: 42,
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            consensus_security_param: 432,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            runtime_sections: Vec::default(),
            protocol_sections: Vec::default(),
        };
        let mut model = Model::new(Config::default(), startup);

        model.handle_message(Message::Telemetry(telemetry!(
            protocols::keepalive::peer::ROUND_TRIP,
            protocols::keepalive::peer::ROUND_TRIP::FIELD_PEER => "1.2.3.4:3001",
            protocols::keepalive::peer::ROUND_TRIP::FIELD_ROUND_TRIP_MICROS => 1_000u64,
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert!(peer.outbound);
        assert!(!peer.inbound);
    }

    #[test]
    fn keyboard_navigation_uses_semicolon_for_focus_and_control_arrows_for_large_steps() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        model.initial_stake_distributions_ready = true;
        for index in 0..40 {
            model.handle_message(named_log(&format!("row-{index}")));
        }
        model.sync_logs();
        model.logs_viewport_rows = 10;

        assert_eq!(model.page, Page::Amaru);
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);
        assert_eq!(model.log_pane_mode, PaneMode::Normal);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE)),
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

        model.handle_key_event(KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE));
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);

        model.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
        assert!(model.log_scroll > 1, "control-up should move by a full scrollbar step");

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.page, Page::Cardano);
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);

        model.handle_key_event(KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE));
        assert_eq!(model.scroll_focus, ScrollFocus::Proposals);

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(model.proposal_pane_mode, PaneMode::Maximized);
    }

    #[test]
    fn config_page_uses_its_own_scroll_focus() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

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
            model.handle_key_event(KeyEvent::new(KeyCode::Char(';'), KeyModifiers::NONE)),
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

    #[test]
    fn shutdown_mode_ignores_follow_up_terminal_input() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        model.enter_shutdown_mode();

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert!(model.is_shutdown_mode());
        assert_eq!(model.interaction_mode, InteractionMode::Shutdown);
        assert_eq!(model.page, Page::Amaru);
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);
    }

    #[test]
    fn splash_screen_ignores_copy_mode_toggle() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        model.initial_stake_distribution_order = vec![1000, 1001];

        assert!(!model.is_ready(Instant::now()));
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert!(!model.is_copy_mode());
        assert_eq!(model.interaction_mode, InteractionMode::Normal);
    }

    fn ready_model() -> Model {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        model.initial_stake_distributions_ready = true;
        model
    }

    fn named_log(name: &str) -> Message {
        Message::Telemetry(telemetry_record(Instant::now(), "amaru::ledger", name, []))
    }

    #[test]
    fn log_filter_menu_filters_log_view_by_regex() {
        let mut model = ready_model();
        model.handle_message(named_log("keep-me"));
        model.handle_message(named_log("drop-me"));

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert!(model.prompt_is_open());
        for character in "keep-me".chars() {
            assert_eq!(
                model.handle_key_event(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
                TerminalEventOutcome::Continue
            );
        }
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert!(!model.prompt_is_open());
        model.sync_logs();

        let names: Vec<_> =
            model.log_view().iter().filter_map(|item| item.record().map(|record| record.name.clone())).collect();
        assert_eq!(names, vec!["keep-me".to_string()]);
    }

    #[test]
    fn wrap_toggle_enables_horizontal_scroll_that_survives_vertical_motion() {
        let mut model = ready_model();
        for index in 0..20 {
            model.handle_message(named_log(&format!("row-{index}")));
        }
        model.sync_logs();
        assert!(model.log_wrap);
        assert_eq!(model.log_hscroll, 0);

        model.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(model.log_hscroll, 0, "wrap on: left/right do not pan");
        assert_eq!(model.scroll_focus, ScrollFocus::Logs);

        model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        model.handle_key_event(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(!model.log_wrap);

        model.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(model.log_hscroll, 8);
        model.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 1);
        assert_eq!(model.log_hscroll, 8, "vertical motion keeps the column offset");
        model.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(model.log_hscroll, 0);
    }

    #[test]
    fn copy_mode_allows_full_log_navigation() {
        let mut model = ready_model();
        for index in 0..40 {
            model.handle_message(named_log(&format!("row-{index}")));
        }
        model.sync_logs();
        model.logs_viewport_rows = 10;

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            TerminalEventOutcome::EnterCopyMode
        );
        assert!(model.is_copy_mode());

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)),
            TerminalEventOutcome::Continue
        );
        assert!(model.log_scroll > 1, "control-up should move by a scrollbar step");

        model.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 0);
        model.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 10);
        model.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 30);
        model.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 0);
        assert!(model.is_copy_mode());
    }

    #[test]
    fn command_menus_require_confirmation_and_escape_returns_to_the_previous_level() {
        let mut model = ready_model();

        model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(model.command_menu, CommandMenu::Logs);
        model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(model.prompt_is_open());
        model.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!model.prompt_is_open());
        assert_eq!(model.command_menu, CommandMenu::Logs);
        model.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(model.command_menu, CommandMenu::Default);

        model.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(model.command_menu, CommandMenu::Quit);
        model.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(model.command_menu, CommandMenu::Default);

        model.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            TerminalEventOutcome::Shutdown
        );
    }

    #[test]
    fn highlight_jumps_between_matching_lines() {
        let mut model = ready_model();
        model.handle_message(named_log("alpha-one"));
        model.handle_message(named_log("skip"));
        model.handle_message(named_log("alpha-two"));
        model.sync_logs();

        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );
        for character in "alpha".chars() {
            model.handle_key_event(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(
            model.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            TerminalEventOutcome::Continue
        );

        let newest = model.log_view().iter().rev().find_map(|item| item.record().map(|record| record.name.clone()));
        assert_eq!(newest.as_deref(), Some("alpha-two"));
        let newest_record = model.log_view().iter().rev().find_map(|item| item.record()).expect("newest log");
        assert!(model.log_record_is_cursor(newest_record));

        model.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let names: Vec<_> = model
            .log_view()
            .iter()
            .filter_map(|item| {
                let record = item.record()?;
                model.log_record_is_cursor(record).then(|| record.name.clone())
            })
            .collect();
        assert_eq!(names, vec!["alpha-one".to_string()]);
    }

    fn named_log_at(name: &str, wall_time: SystemTime) -> Message {
        let mut record = telemetry_record(Instant::now(), "amaru::ledger", name, []);
        record.wall_time = wall_time;
        Message::Telemetry(record)
    }

    fn hour(hour: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(hour * 3_600)
    }

    #[test]
    fn control_arrows_scrub_logs_by_a_scrollbar_step() {
        let mut model = ready_model();
        for index in 0..40 {
            model.handle_message(named_log(&format!("row-{index}")));
        }
        model.sync_logs();
        model.logs_viewport_rows = 10;
        assert_eq!(model.log_scroll, 0);

        model.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
        assert!(model.log_scroll > 1, "control-up should move toward older logs by a scrollbar step");
        model.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 30);
        model.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(model.log_scroll, 0);
    }

    #[test]
    fn at_jumps_to_the_matching_log_time() {
        let mut model = ready_model();
        for hour_of_day in 0..24u64 {
            model.handle_message(named_log_at(&format!("h{hour_of_day}"), hour(hour_of_day)));
        }
        model.sync_logs();

        model.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        model.handle_key_event(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert!(model.prompt_is_open());
        for character in "13:00".chars() {
            model.handle_key_event(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        model.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!model.prompt_is_open());

        let max = 24usize.saturating_sub(10);
        assert_eq!(model.log_scroll, max.saturating_sub(13));
        let visible_start = 24 - 10 - model.log_scroll;
        let name = model.log_view()[visible_start].record().expect("record").name.as_str();
        assert_eq!(name, "h13");
    }

    #[test]
    fn clicking_the_log_scrollbar_jumps_and_starts_a_drag() {
        let mut model = ready_model();
        for index in 0..40 {
            model.handle_message(named_log(&format!("row-{index}")));
        }
        model.sync_logs();

        let views =
            Views { logs_body: Rect::new(0, 10, 40, 10), logs_scrollbar: Rect::new(39, 10, 1, 10), ..Views::default() };

        let outcome = model.handle_terminal_event(
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 39,
                row: 10,
                modifiers: KeyModifiers::NONE,
            }),
            &views,
        );
        assert_eq!(outcome, TerminalEventOutcome::Continue);
        assert!(model.log_scrollbar_focused);
        assert_eq!(model.log_scroll, 30);

        model.handle_terminal_event(
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 39,
                row: 19,
                modifiers: KeyModifiers::NONE,
            }),
            &views,
        );
        assert_eq!(model.log_scroll, 0);
    }
}
