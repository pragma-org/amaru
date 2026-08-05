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

use std::time::{Duration, SystemTime};

use super::*;

impl Model {
    pub fn filtered_logs(&self) -> Vec<&TelemetryRecord> {
        self.logs
            .iter()
            .filter(|record| self.level_filter.allows(record.level) && self.target_filter.allows(&record.target))
            .collect()
    }

    pub fn recent_blocks_count(&self) -> usize {
        self.recent_blocks.len()
    }

    pub fn last_block_elapsed(&self, now: Instant) -> Option<Duration> {
        self.tip.as_ref().map(|tip| now.duration_since(tip.updated_at))
    }

    pub fn network_epoch_at(&self, now: SystemTime) -> Option<u64> {
        self.startup.target_epoch_at(now)
    }

    pub fn sync_progress_at(&self, now: SystemTime) -> Option<(u64, u64, f64)> {
        let tip = self.tip.as_ref()?;
        let target_slot = self.startup.target_slot_at(now)?;
        let current_slot = tip.slot.min(target_slot);

        (target_slot > 0).then_some((current_slot, target_slot, current_slot as f64 / target_slot as f64))
    }

    pub fn slot_throughput(&self) -> Option<f64> {
        let tip = self.tip.as_ref()?;
        let (origin_slot, origin_at) = self.tip_sync_origin?;
        let advanced_slots = tip.slot.saturating_sub(origin_slot);
        let elapsed = tip.updated_at.saturating_duration_since(origin_at);

        (!elapsed.is_zero() && advanced_slots > 0).then_some(advanced_slots as f64 / elapsed.as_secs_f64())
    }

    pub fn sync_eta_at(&self, now: SystemTime) -> Option<Duration> {
        let tip = self.tip.as_ref()?;
        let target_slot = self.startup.target_slot_at(now)?;
        let slot_throughput = self.slot_throughput()?;
        let remaining_slots = target_slot.saturating_sub(tip.slot);

        (remaining_slots > 0 && slot_throughput > 0.0)
            .then(|| Duration::from_secs_f64(remaining_slots as f64 / slot_throughput))
    }

    pub fn recent_transactions_count(&self) -> u64 {
        self.recent_transactions.iter().map(|(_, count)| *count).sum()
    }

    pub fn average_recent_rollback_length(&self) -> Option<f64> {
        let (count, total) = self
            .recent_rollbacks
            .iter()
            .fold((0_u64, 0_u64), |(count, total), (_, length)| (count + 1, total + *length as u64));

        (count > 0).then_some(total as f64 / count as f64)
    }

    pub fn recent_rollback_frequency(&self, now: Instant) -> Option<f64> {
        let span = self.retained_span(
            now,
            self.recent_rollbacks.front().map(|(at, _)| *at),
            self.recent_rollbacks.len(),
            self.config.rollback_sample_capacity,
        );

        (span > Duration::ZERO).then_some(self.recent_rollbacks.len() as f64 / span.as_secs_f64())
    }

    pub fn proposals(&self) -> impl Iterator<Item = &ProposalActivity> {
        self.proposal_order.iter().filter_map(|id| self.proposals_by_id.get(id))
    }

    pub fn sorted_peers(&self) -> Vec<&PeerState> {
        let mut peers = self.peers.values().collect::<Vec<_>>();
        peers.sort_by(|left, right| {
            left.last_rtt_micros
                .unwrap_or(u64::MAX)
                .cmp(&right.last_rtt_micros.unwrap_or(u64::MAX))
                .then_with(|| left.address.cmp(&right.address))
        });
        peers
    }

    pub(crate) fn retained_blocks_span(&self, now: Instant) -> Duration {
        self.retained_span(
            now,
            self.recent_blocks.front().copied(),
            self.recent_blocks.len(),
            self.config.block_sample_capacity,
        )
    }

    pub(crate) fn retained_transactions_span(&self, now: Instant) -> Duration {
        self.retained_span(
            now,
            self.recent_transactions.front().map(|(at, _)| *at),
            self.recent_transactions.len(),
            self.config.transaction_sample_capacity,
        )
    }

    fn retained_span(&self, now: Instant, oldest: Option<Instant>, len: usize, capacity: usize) -> Duration {
        let origin = if len >= capacity { oldest.unwrap_or(self.created_at) } else { self.created_at };
        now.saturating_duration_since(origin)
    }
}
