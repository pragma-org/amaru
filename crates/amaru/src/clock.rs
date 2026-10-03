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

//! Startup check that the operating system is keeping the wall clock in sync with NTP.
//!
//! Slot timing, header validation and block forging all read the wall clock, so a clock that no
//! daemon disciplines (or one whose error bound has grown past [`MAX_CLOCK_ERROR`]) is reported to
//! the operator with a `WARN`.

use std::time::Duration;

use amaru_observability::{info, warn};

pub const MAX_CLOCK_ERROR: Duration = Duration::from_millis(500);

const HINT: &str = "Install and enable an NTP client so the wall clock stays within the tolerance required for slot timing and block forging.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDiscipline {
    pub synchronized: bool,
    pub max_error: Duration,
}

/// Outcome of comparing a [`ClockDiscipline`] against the tolerated error bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockVerdict {
    Ok,
    Unsynchronized,
    MaxErrorExceeded,
}

impl ClockVerdict {
    pub fn reason(self) -> &'static str {
        match self {
            ClockVerdict::Ok => "ok",
            ClockVerdict::Unsynchronized => "unsynchronized",
            ClockVerdict::MaxErrorExceeded => "max_error_exceeded",
        }
    }
}

impl ClockDiscipline {
    pub fn assess(&self, max_error: Duration) -> ClockVerdict {
        if !self.synchronized {
            ClockVerdict::Unsynchronized
        } else if self.max_error > max_error {
            ClockVerdict::MaxErrorExceeded
        } else {
            ClockVerdict::Ok
        }
    }
}

pub fn report(discipline: Result<ClockDiscipline, std::io::Error>, max_error: Duration) {
    let threshold_millis = duration_millis(max_error);
    let discipline = match discipline {
        Ok(discipline) => discipline,
        Err(error) => {
            warn!(setup::clock::UNKNOWN, error = error.to_string());
            return;
        }
    };
    let max_error_millis = duration_millis(discipline.max_error);
    match discipline.assess(max_error) {
        ClockVerdict::Ok => {
            info!(setup::clock::SYNCHRONIZED, max_error_millis, threshold_millis);
        }
        verdict @ (ClockVerdict::Unsynchronized | ClockVerdict::MaxErrorExceeded) => {
            warn!(setup::clock::DRIFT, reason = verdict.reason(), max_error_millis, threshold_millis, hint = HINT);
        }
    }
}

pub fn check() {
    if let Some(discipline) = kernel::read_discipline() {
        report(discipline, MAX_CLOCK_ERROR);
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod kernel {
    use std::time::Duration;

    use super::ClockDiscipline;

    /// Query the kernel NTP state without adjusting it. `None` is never returned on this platform.
    pub fn read_discipline() -> Option<Result<ClockDiscipline, std::io::Error>> {
        // SAFETY: `timex` is a plain C struct for which all-zero bytes is a valid value, and
        // `modes = 0` makes the call read-only, so the kernel only writes into the struct we own.
        let mut timex: libc::timex = unsafe { std::mem::zeroed() };
        let state = unsafe { adjtime(&mut timex) };
        if state == -1 {
            return Some(Err(std::io::Error::last_os_error()));
        }
        let synchronized = state != libc::TIME_ERROR && timex.status & libc::STA_UNSYNC == 0;
        let max_error = Duration::from_micros(u64::try_from(timex.maxerror).unwrap_or(0));
        Some(Ok(ClockDiscipline { synchronized, max_error }))
    }

    #[cfg(target_os = "linux")]
    unsafe fn adjtime(timex: *mut libc::timex) -> libc::c_int {
        unsafe { libc::adjtimex(timex) }
    }

    #[cfg(target_os = "macos")]
    unsafe fn adjtime(timex: *mut libc::timex) -> libc::c_int {
        unsafe { libc::ntp_adjtime(timex) }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod kernel {
    use super::ClockDiscipline;

    pub fn read_discipline() -> Option<Result<ClockDiscipline, std::io::Error>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use amaru_consensus::stages::test_utils::{BufferWriter, install_test_log_capture};
    use amaru_observability::tracing::Level;
    use test_case::test_case;

    use super::*;

    #[test_case(true, 200, ClockVerdict::Ok; "synchronized within bound")]
    #[test_case(true, 500, ClockVerdict::Ok; "synchronized at bound")]
    #[test_case(true, 501, ClockVerdict::MaxErrorExceeded; "synchronized past bound")]
    #[test_case(false, 0, ClockVerdict::Unsynchronized; "unsynchronized with no error estimate")]
    #[test_case(false, 16_000, ClockVerdict::Unsynchronized; "unsynchronized with large error")]
    fn assess(synchronized: bool, max_error_millis: u64, expected: ClockVerdict) {
        let discipline = ClockDiscipline { synchronized, max_error: Duration::from_millis(max_error_millis) };
        assert_eq!(discipline.assess(MAX_CLOCK_ERROR), expected);
    }

    #[test_case(true, 200, Level::INFO, "clock.synchronized", "max_error_millis=200"; "within bound is info")]
    #[test_case(true, 900, Level::WARN, "clock.drift", r#"reason="max_error_exceeded""#; "past bound warns")]
    #[test_case(false, 0, Level::WARN, "clock.drift", r#"reason="unsynchronized""#; "undisciplined warns")]
    fn report_emits_one_trace(synchronized: bool, max_error_millis: u64, level: Level, name: &str, field: &str) {
        let logs = install_test_log_capture(BufferWriter::new());
        let discipline = ClockDiscipline { synchronized, max_error: Duration::from_millis(max_error_millis) };
        report(Ok(discipline), MAX_CLOCK_ERROR);
        logs.logs().assert_and_remove(level, &[name, field, "threshold_millis=500"]).assert_no_remaining_at([
            Level::ERROR,
            Level::WARN,
            Level::INFO,
        ]);
    }

    #[test]
    fn report_warns_when_kernel_state_is_unreadable() {
        let logs = install_test_log_capture(BufferWriter::new());
        report(Err(std::io::Error::other("operation not permitted")), MAX_CLOCK_ERROR);
        logs.logs()
            .assert_and_remove(Level::WARN, &["clock.unknown", "operation not permitted"])
            .assert_no_remaining_at([Level::ERROR, Level::WARN, Level::INFO]);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn kernel_discipline_is_readable() {
        let discipline = kernel::read_discipline().expect("supported platform").expect("read-only query succeeds");
        eprintln!("{discipline:?} -> {:?}", discipline.assess(MAX_CLOCK_ERROR));
    }
}
