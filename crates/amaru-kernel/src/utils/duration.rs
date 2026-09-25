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

pub fn parse(raw: &str) -> Result<Duration, String> {
    let split = raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len());
    let (digits, unit) = raw.split_at(split);

    if digits.is_empty() {
        return Err(format!("invalid duration '{raw}': missing number"));
    }

    let value = digits.parse::<u64>().map_err(|_| format!("invalid duration '{raw}': invalid number"))?;

    let duration = match unit {
        "ns" | "nanos" => Duration::from_nanos(value),
        "us" | "μs" | "micros" => Duration::from_micros(value),
        "ms" | "millis" => Duration::from_millis(value),
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

pub fn format(duration: &Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        let micros = duration.as_micros();
        if micros == 0 {
            format!("{}ns", duration.as_nanos())
        } else if micros < 1000 {
            format!("{}μs", micros)
        } else if micros < 1_000_000 {
            format!("{}ms", duration.as_millis())
        } else {
            format!("{seconds}s")
        }
    } else if seconds < 3_600 {
        format!("{}min", seconds / 60)
    } else {
        format!("{}h", seconds / 3_600)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use test_case::test_case;

    #[test_case("27ns"  => Some(Duration::from_nanos(27)))]
    #[test_case("82us"  => Some(Duration::from_micros(82)))]
    #[test_case("82μs"  => Some(Duration::from_micros(82)))]
    #[test_case("100ms" => Some(Duration::from_millis(100)))]
    #[test_case("30"    => Some(Duration::from_secs(30)))]
    #[test_case("30s"   => Some(Duration::from_secs(30)))]
    #[test_case("1min"  => Some(Duration::from_secs(60)))]
    #[test_case("6h"    => Some(Duration::from_secs(21_600)))]
    fn parses_duration(s: &str) -> Option<Duration> {
        super::parse(s).ok()
    }

    #[test_case(Duration::from_micros(999)  => "999μs")]
    #[test_case(Duration::from_nanos(27)     => "27ns")]
    #[test_case(Duration::from_millis(2300) => "2s")]
    #[test_case(Duration::from_millis(42)   => "42ms")]
    #[test_case(Duration::from_secs(30)     => "30s")]
    #[test_case(Duration::from_secs(300)    => "5min")]
    #[test_case(Duration::from_secs(7_200)  => "2h")]
    fn formats_duration(duration: Duration) -> String {
        super::format(&duration)
    }
}
