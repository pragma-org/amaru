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

use amaru_observability::{
    RecordFields,
    amaru::{bootstrap, consensus, ledger, mempool, protocols},
};

use super::*;
use crate::events::{Message, TelemetryRecord};

impl Model {
    pub fn handle_message(&mut self, message: Message) {
        match message {
            Message::Telemetry(record) => {
                self.prune_stale_peers(record.at);
                self.record_telemetry(record);
            }
            Message::Metrics(record) => {
                self.prune_stale_peers(record.at);
                self.record_metrics(record);
            }
        }
    }

    fn prune_stale_peers(&mut self, now: Instant) {
        self.peers.retain(|_, peer| !peer.is_stale(now, self.config.peer_inactivity_timeout));
    }

    fn record_telemetry(&mut self, record: TelemetryRecord) {
        self.update_state(TelemetryEvent::from_record(&record), &record);
        self.logs.push(record, &self.config, self.level_filter, self.target_filter);
    }

    fn update_state(&mut self, event: Option<TelemetryEvent>, record: &TelemetryRecord) {
        let Some(event) = event else {
            return;
        };

        match event {
            TelemetryEvent::BlockAdopt => {
                self.update_catch_up(record);
            }
            TelemetryEvent::RollForward => self.push_recent_block(),
            TelemetryEvent::TransactionValidate => self.push_recent_transaction_count(1),
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
            TelemetryEvent::StateSwitchToFork => {
                self.push_recent_rollback(ledger::state::SWITCH_TO_FORK::rollback_length(record), record.at)
            }
            TelemetryEvent::HeaderLifecycle => self.update_peer_header_lifecycle(record),
            TelemetryEvent::KeepaliveRoundTrip => self.update_peer_rtt(record),
            TelemetryEvent::PeerConnected => self.update_peer_connected(record),
            TelemetryEvent::PeerDisconnected => self.update_peer_disconnected(record),
            TelemetryEvent::PeerResolved => self.update_peer_resolved(record),
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
            TelemetryEvent::ProposalActive => self.push_active_proposal(record),
            TelemetryEvent::ProposalDrop => self.push_dropped_proposal(record),
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

        self.tip_sync_origin.get_or_insert((tip.slot, tip.updated_at));
        self.tip = Some(tip);
    }

    fn update_catch_up(&mut self, record: &TelemetryRecord) {
        let catching_up_by_height =
            consensus::tip::ADOPT::max_block_height(record) > consensus::tip::ADOPT::block_height(record);
        let catching_up_by_slot = self
            .startup
            .is_near_target_slot_at(consensus::tip::ADOPT::slot(record), record.wall_time)
            .is_some_and(|is_near_tip| !is_near_tip);
        let catching_up = catching_up_by_height || catching_up_by_slot;

        if catching_up {
            for peer in self.peers.values_mut() {
                peer.clear_slot_start_to_header();
            }
        }

        self.catching_up = catching_up;
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

    fn update_peer_resolved(&mut self, record: &TelemetryRecord) {
        let address = protocols::peer_selection::peer::RESOLVED::peer(record);
        let candidate = protocols::peer_selection::peer::RESOLVED::candidate(record);
        if candidate == address {
            return;
        }
        self.resolved_candidates.insert(address.to_string(), candidate.to_string());
        if let Some(peer) = self.peers.get_mut(address) {
            peer.candidate = Some(candidate.to_string());
        }
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
        if record.str(consensus::perf::header::LIFECYCLE::FIELD_OUTCOME) != Some("valid") {
            return;
        }

        let Some(peer) = consensus::perf::header::LIFECYCLE::peer(record) else {
            return;
        };

        let slot_start_to_header_micros = (!self.catching_up)
            .then(|| consensus::perf::header::LIFECYCLE::slot_start_to_header_micros(record))
            .flatten();
        let query_header_micros = consensus::perf::header::LIFECYCLE::block_fetch_wait_micros(record);
        let get_block_micros = consensus::perf::header::LIFECYCLE::block_fetch_micros(record);
        let adopt_block_micros = consensus::perf::header::LIFECYCLE::forward_micros(record)
            .zip(query_header_micros)
            .zip(get_block_micros)
            .map(|((forward_micros, query_header_micros), get_block_micros)| {
                forward_micros.saturating_sub(query_header_micros.saturating_add(get_block_micros))
            });

        let capacity = self.config.peer_timing_capacity;
        let peer = self.peer_mut(peer, record.at);
        peer.record_header_lifecycle(
            record.at,
            capacity,
            slot_start_to_header_micros,
            query_header_micros,
            get_block_micros,
            adopt_block_micros,
        );
    }

    fn peer_mut(&mut self, address: impl ToString, updated_at: Instant) -> &mut PeerState {
        let address = address.to_string();
        let candidate = self.resolved_candidates.get(&address).cloned();
        self.peers.entry(address.clone()).or_insert_with(|| {
            let mut peer = PeerState::new(address, updated_at);
            peer.candidate = candidate;
            peer
        })
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
}
