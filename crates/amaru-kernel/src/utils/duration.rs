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

/// Measure the time in μs since a previous checkpoint while refreshing the checkpoint.
pub fn elapsed_and_reset(meter: &mut Instant) -> u64 {
    let now = Instant::now();
    let us = now.saturating_duration_since(*meter).as_micros() as u64;
    *meter = now;
    us
}

pub fn parse_duration(raw: &str) -> Result<Duration, String> {
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

pub fn format_duration_short(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}min", seconds / 60)
    } else {
        format!("{}h", seconds / 3_600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duration() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("1min").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("6h").unwrap(), Duration::from_secs(21_600));
    }

    #[test]
    fn formats_duration() {
        assert_eq!(format_duration_short(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration_short(Duration::from_secs(300)), "5min");
        assert_eq!(format_duration_short(Duration::from_secs(7_200)), "2h");
    }
}
