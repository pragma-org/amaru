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

#[derive(Debug, Clone, PartialEq)]
pub struct SystemSample {
    pub at: Instant,
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    pub virtual_bytes: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub disk_live_read_bytes: u64,
    pub disk_live_write_bytes: u64,
    pub processes_live_read_bytes: u64,
    pub processes_live_write_bytes: u64,
}
