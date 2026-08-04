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

use crate::{
    config::{Config, TimeWindow},
    events::{SystemSample, TelemetryRecord},
    startup::StartupContext,
};

mod governance_summary;
mod initial_stake_distribution_state;
mod interaction;
mod interaction_mode;
mod level_filter;
mod mempool_state;
mod metrics_update;
mod page;
mod pane_mode;
mod peer_state;
mod proposal_activity;
mod queries;
mod scroll_focus;
mod stake_snapshot_state;
mod target_filter;
mod telemetry_event;
mod telemetry_update;
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
    pub catching_up: bool,
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
            catching_up: true,
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
        events::{FieldValue, HostSample, Message, TelemetryKind, TelemetryRecord},
        model::telemetry_event::{LEDGER_TARGET, PROTOCOLS_TARGET},
        startup::ProcessInfo,
    };

    fn telemetry(target: &str, name: &str, fields: &[(&str, FieldValue)]) -> TelemetryRecord {
        telemetry_at(Instant::now(), target, name, fields)
    }

    fn telemetry_at(at: Instant, target: &str, name: &str, fields: &[(&str, FieldValue)]) -> TelemetryRecord {
        TelemetryRecord {
            kind: TelemetryKind::Event,
            level: Level::INFO,
            target: target.into(),
            name: name.into(),
            at,
            wall_time: std::time::SystemTime::UNIX_EPOCH,
            fields: fields.iter().map(|(name, value)| ((*name).into(), value.clone())).collect(),
        }
    }

    fn metric(at: Instant, event: MetricsEvent) -> Message {
        Message::Metrics(crate::events::MetricRecord { at, event })
    }

    fn host_sample(at: Instant, interval: Duration) -> Message {
        Message::HostSample(HostSample {
            at,
            interval,
            memory_used_bytes: 100_000,
            memory_total_bytes: 200_000,
            processes_live_read_bytes: 1_500,
            processes_live_write_bytes: 2_500,
        })
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
                node_start_time_seconds: 0,
                cpu_ticks: 0,
                network_read_bytes: 0,
                network_written_bytes: 0,
                runtime_seconds: 1,
                cpu_percent: 12.5,
                process_memory_bytes: 10_000,
                rss_bytes: 9_000,
                virtual_bytes: 12_000,
                disk_read_bytes: 300,
                disk_write_bytes: 400,
                disk_live_read_bytes: 30,
                disk_live_write_bytes: 40,
                open_files: 5,
            }),
        ));
        model.handle_message(host_sample(at + Duration::from_secs(5), Duration::from_secs(5)));

        assert_eq!(model.blocks_in_window(at + Duration::from_secs(1)), 1);
        assert_eq!(model.transactions_in_window(at + Duration::from_secs(1)), 7);
        assert_eq!(model.system_samples.back().map(|sample| sample.process_memory_bytes), Some(10_000));
        assert_eq!(model.system_samples.back().map(|sample| sample.memory_total_bytes), Some(200_000));
        assert_eq!(model.system_samples.back().map(|sample| sample.processes_live_read_bytes), Some(300));
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
    fn sorts_peers_by_ascending_rtt() {
        let mut model = Model::new(Config::default(), startup_context());
        let now = Instant::now();

        let mut fast = PeerState::new("fast.example:3001".into(), false, now);
        fast.last_rtt_micros = Some(5_000);

        let mut unknown = PeerState::new("unknown.example:3001".into(), false, now);
        unknown.last_rtt_micros = None;

        let mut slow = PeerState::new("slow.example:3001".into(), false, now);
        slow.last_rtt_micros = Some(15_000);

        model.peers.insert(fast.address.clone(), fast);
        model.peers.insert(unknown.address.clone(), unknown);
        model.peers.insert(slow.address.clone(), slow);

        let peers = model.sorted_peers();
        let addresses = peers.iter().map(|peer| peer.address.as_str()).collect::<Vec<_>>();

        assert_eq!(addresses, vec!["fast.example:3001", "slow.example:3001", "unknown.example:3001"]);
    }

    #[test]
    fn prunes_stale_peers_outside_the_maximum_window() {
        let mut model = Model::new(Config::default(), startup_context());
        let max_window = model.max_window();
        let stale_at = Instant::now();
        let now = stale_at + max_window + Duration::from_secs(1);

        model.peers.insert("stale.example:3001".into(), PeerState::new("stale.example:3001".into(), false, stale_at));
        model.peers.insert(
            "recent.example:3001".into(),
            PeerState::new("recent.example:3001".into(), false, now - Duration::from_secs(1)),
        );

        model.handle_message(host_sample(now, Duration::from_secs(1)));

        assert!(!model.peers.contains_key("stale.example:3001"));
        assert!(model.peers.contains_key("recent.example:3001"));
    }

    #[test]
    fn tracks_peer_header_lifecycle_means() {
        let mut model = Model::new(Config::default(), startup_context());
        let now = model.created_at;

        model.handle_message(Message::Telemetry(telemetry_at(
            now,
            "amaru::consensus",
            "tip.adopt",
            &[
                ("slot", FieldValue::U64(1)),
                ("header_hash", FieldValue::String("abc".into())),
                ("block_height", FieldValue::U64(10)),
                ("max_block_height", FieldValue::U64(10)),
                ("suppressed", FieldValue::U64(0)),
            ],
        )));

        model.handle_message(Message::Telemetry(telemetry_at(
            now,
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("slot_start_to_header_micros", FieldValue::U64(9_000)),
                ("block_fetch_wait_micros", FieldValue::U64(2_000)),
                ("block_fetch_micros", FieldValue::U64(5_000)),
                ("forward_micros", FieldValue::U64(11_000)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry_at(
            now + Duration::from_secs(1),
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("slot_start_to_header_micros", FieldValue::U64(15_000)),
                ("block_fetch_wait_micros", FieldValue::U64(4_000)),
                ("block_fetch_micros", FieldValue::U64(7_000)),
                ("forward_micros", FieldValue::U64(15_000)),
            ],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(
            peer.mean_slot_start_to_header_micros(now + Duration::from_secs(1), model.current_window()),
            Some(12_000)
        );
        assert_eq!(peer.mean_query_header_micros(now + Duration::from_secs(1), model.current_window()), Some(3_000));
        assert_eq!(peer.mean_get_block_micros(now + Duration::from_secs(1), model.current_window()), Some(6_000));
        assert_eq!(peer.mean_adopt_block_micros(now + Duration::from_secs(1), model.current_window()), Some(4_000));
    }

    #[test]
    fn peer_header_lifecycle_means_follow_the_selected_window() {
        let config = Config::default().with_windows(vec![TimeWindow::from_secs(5), TimeWindow::from_secs(20)]);
        let mut model = Model::new(config, startup_context());
        let now = model.created_at;

        model.handle_message(Message::Telemetry(telemetry_at(
            now,
            "amaru::consensus",
            "tip.adopt",
            &[
                ("slot", FieldValue::U64(1)),
                ("header_hash", FieldValue::String("abc".into())),
                ("block_height", FieldValue::U64(10)),
                ("max_block_height", FieldValue::U64(10)),
                ("suppressed", FieldValue::U64(0)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry_at(
            now,
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("slot_start_to_header_micros", FieldValue::U64(9_000)),
                ("block_fetch_wait_micros", FieldValue::U64(2_000)),
                ("block_fetch_micros", FieldValue::U64(5_000)),
                ("forward_micros", FieldValue::U64(11_000)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry_at(
            now + Duration::from_secs(10),
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("slot_start_to_header_micros", FieldValue::U64(15_000)),
                ("block_fetch_wait_micros", FieldValue::U64(4_000)),
                ("block_fetch_micros", FieldValue::U64(7_000)),
                ("forward_micros", FieldValue::U64(15_000)),
            ],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_query_header_micros(now + Duration::from_secs(10), model.current_window()), Some(4_000));
        assert_eq!(
            peer.mean_slot_start_to_header_micros(now + Duration::from_secs(10), model.current_window()),
            Some(15_000)
        );
        assert_eq!(peer.mean_query_header_micros(now + Duration::from_secs(10), Duration::from_secs(20)), Some(3_000));
        assert_eq!(
            peer.mean_slot_start_to_header_micros(now + Duration::from_secs(10), Duration::from_secs(20)),
            Some(12_000)
        );
    }

    #[test]
    fn slot_start_timing_is_hidden_while_catching_up() {
        let mut model = Model::new(Config::default(), startup_context());
        let now = model.created_at;

        model.handle_message(Message::Telemetry(telemetry_at(
            now,
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("slot_start_to_header_micros", FieldValue::U64(9_000)),
                ("block_fetch_wait_micros", FieldValue::U64(2_000)),
                ("block_fetch_micros", FieldValue::U64(5_000)),
                ("forward_micros", FieldValue::U64(11_000)),
            ],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(peer.mean_slot_start_to_header_micros(now, model.current_window()), None);

        model.handle_message(Message::Telemetry(telemetry_at(
            now + Duration::from_secs(1),
            "amaru::consensus",
            "tip.adopt",
            &[
                ("slot", FieldValue::U64(1)),
                ("header_hash", FieldValue::String("abc".into())),
                ("block_height", FieldValue::U64(10)),
                ("max_block_height", FieldValue::U64(100)),
                ("suppressed", FieldValue::U64(0)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry_at(
            now + Duration::from_secs(2),
            "amaru::consensus",
            "tip.adopt",
            &[
                ("slot", FieldValue::U64(1)),
                ("header_hash", FieldValue::String("abc".into())),
                ("block_height", FieldValue::U64(100)),
                ("max_block_height", FieldValue::U64(100)),
                ("suppressed", FieldValue::U64(0)),
            ],
        )));
        model.handle_message(Message::Telemetry(telemetry_at(
            now + Duration::from_secs(3),
            "amaru::consensus",
            "perf.header.lifecycle",
            &[
                ("peer", FieldValue::String("1.2.3.4:3001".into())),
                ("slot_start_to_header_micros", FieldValue::U64(3_000)),
                ("block_fetch_wait_micros", FieldValue::U64(1_000)),
                ("block_fetch_micros", FieldValue::U64(2_000)),
                ("forward_micros", FieldValue::U64(5_000)),
            ],
        )));

        let peer = model.peers.get("1.2.3.4:3001").expect("peer must exist");
        assert_eq!(
            peer.mean_slot_start_to_header_micros(now + Duration::from_secs(3), model.current_window()),
            Some(3_000)
        );
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
