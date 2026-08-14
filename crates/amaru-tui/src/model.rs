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

mod exponential_moving_average;
mod governance_summary;
mod initial_stake_distribution_state;
mod interaction;
mod interaction_mode;
mod level_filter;
mod log_buffer;
mod mempool_state;
mod metrics_update;
mod page;
mod pane_mode;
mod peer_state;
mod proposal_activity;
mod queries;
mod rate_counter;
mod scroll_focus;
mod stake_snapshot_state;
mod target_filter;
mod telemetry_event;
mod telemetry_update;
mod terminal_event_outcome;
mod tip_state;

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
    pub catching_up: bool,
    pub log_scroll: usize,
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
    pub logs: LogBuffer,
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
            log_pane_mode: PaneMode::Normal,
            peer_pane_mode: PaneMode::Normal,
            proposal_pane_mode: PaneMode::Normal,
            scroll_focus: ScrollFocus::Logs,
            level_filter: LevelFilter::Debug,
            target_filter: TargetFilter::All,
            catching_up: true,
            log_scroll: 0,
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
            logs: LogBuffer::default(),
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
    use std::time::Duration;

    use amaru_metrics::{MetricsEvent, system::SystemMetrics};
    use amaru_observability::amaru::{consensus, ledger, protocols};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tracing::Level;

    use super::*;
    use crate::{
        events::{FieldValue, Message, TelemetryRecord},
        startup::ProcessInfo,
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
                ledger::transaction::VALIDATE::FIELD_TRANSACTION_ID => format!("tx-{index}"),
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
    fn keyboard_navigation_uses_arrows_for_focus_and_enter_for_pane_toggle() {
        let mut model = Model::new(Config::default(), fixture_startup_context());

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
}
