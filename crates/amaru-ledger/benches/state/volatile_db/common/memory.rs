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

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};

#[expect(clippy::panic, reason = "non-production code")]
pub fn current_process_memory() -> u64 {
    let pid = get_current_pid().unwrap_or_else(|e| panic!("unable to get current pid for memory benchmark: {e}"));
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::everything());
    system
        .process(pid)
        .map(|process| process.memory())
        .unwrap_or_else(|| panic!("unable to read process memory for pid {pid:?}"))
}

/// Return the RSS memory delta, in MB, from before and after executing a task.
pub fn rss_delta<A>(task: impl FnOnce() -> A) -> (A, u64) {
    let rss_before = current_process_memory();
    let result = task();
    let rss_after = current_process_memory();
    (result, (rss_after.saturating_sub(rss_before) as f64 / (1024.0 * 1024.0)).round() as u64)
}
