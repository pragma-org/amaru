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

use std::{collections::VecDeque, time::Instant};

use amaru_metrics::MetricsEvent;

use super::*;
use crate::events::{MetricRecord, SystemSample};

impl Model {
    pub(crate) fn record_metrics(&mut self, record: MetricRecord) {
        if let MetricsEvent::SystemMetrics(metrics) = record.event {
            self.system_sample = Some(SystemSample {
                at: record.at,
                cpu_percent: metrics.cpu_percent,
                process_memory_bytes: metrics.process_memory_bytes,
                rss_bytes: metrics.process_memory_live_resident,
                virtual_bytes: metrics.process_memory_available_virtual,
                memory_used_bytes: metrics.memory_used_bytes,
                memory_total_bytes: metrics.memory_total_bytes,
                disk_read_bytes: metrics.disk_read_bytes,
                disk_write_bytes: metrics.disk_write_bytes,
                disk_live_read_bytes: metrics.disk_live_read_bytes,
                disk_live_write_bytes: metrics.disk_live_write_bytes,
                host_live_read_bytes: metrics.host_live_read_bytes,
                host_live_write_bytes: metrics.host_live_write_bytes,
            });
        }
    }

    pub(crate) fn push_recent_transaction_count(&mut self, at: Instant, tx_count: u64) {
        self.transaction_rate.record(at, tx_count);
    }

    pub(crate) fn push_recent_block(&mut self, at: Instant) {
        self.block_rate.record(at, 1);
    }

    pub(crate) fn push_recent_rollback(&mut self, rollback_length: usize, at: Instant) {
        self.recent_rollbacks.push_back((at, rollback_length));
        prune_recent_count(&mut self.recent_rollbacks, self.config.rollback_sample_capacity);
    }
}

pub(crate) fn prune_recent_count<T>(entries: &mut VecDeque<T>, capacity: usize) {
    while entries.len() > capacity {
        entries.pop_front();
    }
}
