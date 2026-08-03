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

use std::{fmt, str::FromStr, time::Duration};

use amaru_kernel::utils::duration::{format_duration_short, parse_duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeWindow(Duration);

impl TimeWindow {
    pub const fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

impl fmt::Display for TimeWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_duration_short(self.0))
    }
}

impl FromStr for TimeWindow {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        parse_duration(raw).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub windows: Vec<TimeWindow>,
    pub log_capacity: usize,
    pub proposal_capacity: usize,
    pub sample_interval: Duration,
    pub tick_interval: Duration,
    pub splash_timeout: Duration,
    pub channel_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            windows: vec![TimeWindow::from_secs(300), TimeWindow::from_secs(3_600), TimeWindow::from_secs(21_600)],
            log_capacity: 1_024,
            proposal_capacity: 24,
            sample_interval: Duration::from_secs(1),
            tick_interval: Duration::from_millis(250),
            splash_timeout: Duration::from_secs(3),
            channel_capacity: 4_096,
        }
    }
}

impl Config {
    pub fn with_windows(self, windows: Vec<TimeWindow>) -> Self {
        Self { windows, ..self }
    }
}

pub fn format_windows(windows: &[TimeWindow]) -> String {
    windows.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_time_window() {
        assert_eq!("30s".parse::<TimeWindow>().unwrap(), TimeWindow::from_secs(30));
        assert_eq!("1min".parse::<TimeWindow>().unwrap(), TimeWindow::from_secs(60));
        assert_eq!("6h".parse::<TimeWindow>().unwrap(), TimeWindow::from_secs(21_600));
    }

    #[test]
    fn formats_time_window() {
        assert_eq!(TimeWindow::from_secs(30).to_string(), "30s");
        assert_eq!(TimeWindow::from_secs(300).to_string(), "5min");
        assert_eq!(TimeWindow::from_secs(7_200).to_string(), "2h");
    }
}
