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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub windows: Vec<Duration>,
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
            windows: vec![Duration::from_secs(60), Duration::from_secs(600), Duration::from_secs(3600)],
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
    pub fn with_windows(self, windows: Vec<Duration>) -> Self {
        Self { windows, ..self }
    }
}

pub fn parse_windows(raw: &str) -> Result<Vec<Duration>, String> {
    let mut windows = Vec::new();

    for item in raw.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        windows.push(parse_duration(item)?);
    }

    if windows.is_empty() {
        return Err("at least one window must be provided".into());
    }

    windows.sort_unstable();
    windows.dedup();

    Ok(windows)
}

pub fn format_duration_short(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3_600)
    }
}

fn parse_duration(raw: &str) -> Result<Duration, String> {
    let split = raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len());
    let (digits, unit) = raw.split_at(split);

    if digits.is_empty() {
        return Err(format!("invalid duration '{raw}': missing number"));
    }

    let value = digits.parse::<u64>().map_err(|_| format!("invalid duration '{raw}': invalid number"))?;

    let duration = match unit {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => Duration::from_secs(value),
        "m" | "min" | "mins" | "minute" | "minutes" => Duration::from_secs(value.saturating_mul(60)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Duration::from_secs(value.saturating_mul(3_600)),
        _ => return Err(format!("invalid duration '{raw}': unsupported unit '{unit}'")),
    };

    if duration.is_zero() {
        return Err(format!("invalid duration '{raw}': zero is not allowed"));
    }

    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_list() {
        assert_eq!(
            parse_windows("30s, 1min, 6h").unwrap(),
            vec![Duration::from_secs(30), Duration::from_secs(60), Duration::from_secs(21_600)]
        );
    }

    #[test]
    fn rejects_empty_list() {
        assert!(parse_windows(" , ").is_err());
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration_short(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration_short(Duration::from_secs(300)), "5m");
        assert_eq!(format_duration_short(Duration::from_secs(7_200)), "2h");
    }
}
