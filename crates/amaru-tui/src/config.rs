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

use std::time::Duration;

use tracing::Level;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub debug_log_capacity: usize,
    pub info_log_capacity: usize,
    pub warn_log_capacity: usize,
    pub error_log_capacity: usize,
    pub block_sample_capacity: usize,
    pub transaction_sample_capacity: usize,
    pub rollback_sample_capacity: usize,
    pub peer_timing_capacity: usize,
    pub peer_inactivity_timeout: Duration,
    pub proposal_capacity: usize,
    pub sample_interval: Duration,
    pub tick_interval: Duration,
    pub splash_timeout: Duration,
    pub channel_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            debug_log_capacity: 1_000,
            info_log_capacity: 500,
            warn_log_capacity: 100,
            error_log_capacity: 100,
            block_sample_capacity: 100,
            transaction_sample_capacity: 100,
            rollback_sample_capacity: 100,
            peer_timing_capacity: 100,
            peer_inactivity_timeout: Duration::from_secs(600),
            proposal_capacity: 24,
            sample_interval: Duration::from_secs(1),
            tick_interval: Duration::from_millis(250),
            splash_timeout: Duration::from_secs(3),
            channel_capacity: 4_096,
        }
    }
}

impl Config {
    pub fn log_capacity_for(&self, level: Level) -> usize {
        match level {
            Level::TRACE | Level::DEBUG => self.debug_log_capacity,
            Level::INFO => self.info_log_capacity,
            Level::WARN => self.warn_log_capacity,
            Level::ERROR => self.error_log_capacity,
        }
    }
}
