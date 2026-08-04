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

use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct HostSample {
    pub at: Instant,
    pub interval: Duration,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub processes_live_read_bytes: u64,
    pub processes_live_write_bytes: u64,
}

impl HostSample {
    pub fn processes_live_read_bytes_per_second(&self) -> u64 {
        bytes_per_second(self.processes_live_read_bytes, self.interval)
    }

    pub fn processes_live_write_bytes_per_second(&self) -> u64 {
        bytes_per_second(self.processes_live_write_bytes, self.interval)
    }
}

fn bytes_per_second(bytes: u64, interval: Duration) -> u64 {
    let seconds = interval.as_secs_f64();
    if seconds == 0.0 { 0 } else { (bytes as f64 / seconds).round() as u64 }
}
