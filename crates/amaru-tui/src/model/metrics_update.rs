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
    collections::VecDeque,
    time::{Duration, Instant},
};

use amaru_metrics::{LedgerMetrics, MempoolMetrics, MetricsEvent, SystemMetrics};

use super::*;
use crate::events::{HostSample, MetricRecord, SystemSample};

impl Model {
    pub fn push_system_sample(&mut self, sample: SystemSample) {
        self.system_samples.push_back(sample);
        while self.system_samples.len() > self.system_capacity() {
            self.system_samples.pop_front();
        }
    }

    pub(crate) fn record_metrics(&mut self, record: MetricRecord) {
        match record.event {
            MetricsEvent::LedgerMetrics(metrics) => self.record_ledger_metrics(record.at, metrics),
            MetricsEvent::MempoolMetrics(metrics) => self.record_mempool_metrics(record.at, metrics),
            MetricsEvent::SystemMetrics(metrics) => self.record_system_metrics(record.at, metrics),
            MetricsEvent::ProtocolMetrics(_) | MetricsEvent::ConsensusMetrics(_) => {}
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
        let (
            memory_used_bytes,
            memory_total_bytes,
            process_memory_bytes_override,
            host_live_read_bytes,
            host_live_write_bytes,
        ) = self.latest_host_metrics();

        self.push_system_sample(SystemSample {
            at,
            cpu_percent: metrics.cpu_percent,
            process_memory_bytes: process_memory_bytes_override.unwrap_or(metrics.process_memory_bytes),
            rss_bytes: metrics.rss_bytes,
            virtual_bytes: metrics.virtual_bytes,
            memory_used_bytes,
            memory_total_bytes,
            disk_read_bytes: metrics.disk_read_bytes,
            disk_write_bytes: metrics.disk_write_bytes,
            disk_live_read_bytes: metrics.disk_live_read_bytes,
            disk_live_write_bytes: metrics.disk_live_write_bytes,
            host_live_read_bytes,
            host_live_write_bytes,
        });
    }

    pub(crate) fn record_host_sample(&mut self, sample: HostSample) {
        let (
            cpu_percent,
            process_memory_bytes,
            rss_bytes,
            virtual_bytes,
            disk_read_bytes,
            disk_write_bytes,
            disk_live_read_bytes,
            disk_live_write_bytes,
        ) = self.latest_process_metrics();

        self.push_system_sample(SystemSample {
            at: sample.at,
            cpu_percent,
            process_memory_bytes: sample.process_memory_bytes.unwrap_or(process_memory_bytes),
            rss_bytes,
            virtual_bytes,
            memory_used_bytes: sample.memory_used_bytes,
            memory_total_bytes: sample.memory_total_bytes,
            disk_read_bytes,
            disk_write_bytes,
            disk_live_read_bytes,
            disk_live_write_bytes,
            host_live_read_bytes: sample.host_live_read_bytes_per_second(),
            host_live_write_bytes: sample.host_live_write_bytes_per_second(),
        });
    }

    pub(crate) fn push_recent_block(&mut self, at: Instant) {
        let max_window = self.max_window();
        self.recent_blocks.push_back(at);
        prune_recent(&mut self.recent_blocks, at, max_window);
    }

    pub(crate) fn push_recent_rollback(&mut self, rollback_length: usize, at: Instant) {
        let max_window = self.max_window();
        self.recent_rollbacks.push_back((at, rollback_length));

        while self.recent_rollbacks.front().is_some_and(|(entry_at, _)| at.duration_since(*entry_at) > max_window) {
            self.recent_rollbacks.pop_front();
        }
    }
}

pub(crate) fn prune_recent(entries: &mut VecDeque<Instant>, now: Instant, max_window: Duration) {
    while entries.front().is_some_and(|at| now.duration_since(*at) > max_window) {
        entries.pop_front();
    }
}

impl Model {
    fn latest_host_metrics(&self) -> (u64, u64, Option<u64>, u64, u64) {
        self.system_samples
            .back()
            .map(|sample| {
                (
                    sample.memory_used_bytes,
                    sample.memory_total_bytes,
                    Some(sample.process_memory_bytes),
                    sample.host_live_read_bytes,
                    sample.host_live_write_bytes,
                )
            })
            .unwrap_or((0, 0, None, 0, 0))
    }

    fn latest_process_metrics(&self) -> (f64, u64, u64, u64, u64, u64, u64, u64) {
        self.system_samples
            .back()
            .map(|sample| {
                (
                    sample.cpu_percent,
                    sample.process_memory_bytes,
                    sample.rss_bytes,
                    sample.virtual_bytes,
                    sample.disk_read_bytes,
                    sample.disk_write_bytes,
                    sample.disk_live_read_bytes,
                    sample.disk_live_write_bytes,
                )
            })
            .unwrap_or((0.0, 0, 0, 0, 0, 0, 0, 0))
    }
}
