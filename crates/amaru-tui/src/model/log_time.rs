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

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PARSE_ERROR: &str = "expected HH:MM[:SS] or YYYY-MM-DD[ HH:MM[:SS]]";

/// A UTC time the operator asked to jump to in the log pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeJump {
    TimeOfDay { hour: u8, minute: u8, second: u8 },
    Date { year: u16, month: u8, day: u8 },
    DateTime { year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8 },
}

impl TimeJump {
    pub fn parse(input: &str) -> Result<Self, String> {
        parse_time_jump(input)
    }

    /// Absolute instant to search for in `[oldest, newest]`.
    pub fn resolve(self, oldest: SystemTime, newest: SystemTime) -> SystemTime {
        let (lo, hi) = if oldest <= newest { (oldest, newest) } else { (newest, oldest) };
        match self {
            Self::TimeOfDay { hour, minute, second } => resolve_time_of_day(hour, minute, second, lo, hi),
            Self::Date { year, month, day } => civil_to_system_time(year, month, day, 0, 0, 0).unwrap_or(lo),
            Self::DateTime { year, month, day, hour, minute, second } => {
                civil_to_system_time(year, month, day, hour, minute, second).unwrap_or(lo)
            }
        }
    }
}

fn parse_time_jump(input: &str) -> Result<TimeJump, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(PARSE_ERROR.to_string());
    }

    if let Some((date, rest)) = split_date_prefix(input) {
        let (year, month, day) = date;
        if rest.is_empty() {
            return Ok(TimeJump::Date { year, month, day });
        }
        let (hour, minute, second) = parse_hms(rest)?;
        return Ok(TimeJump::DateTime { year, month, day, hour, minute, second });
    }

    let (hour, minute, second) = parse_hms(input)?;
    Ok(TimeJump::TimeOfDay { hour, minute, second })
}

fn split_date_prefix(input: &str) -> Option<((u16, u8, u8), &str)> {
    let (date_part, rest) = if let Some((date, time)) = input.split_once('T') {
        (date, time)
    } else if let Some((date, time)) = input.split_once(' ') {
        (date, time)
    } else {
        (input, "")
    };

    let mut parts = date_part.split('-');
    let year = parts.next()?.parse::<u16>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let day = parts.next()?.parse::<u8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    validate_date(year, month, day).ok()?;
    Some(((year, month, day), rest.trim()))
}

fn parse_hms(input: &str) -> Result<(u8, u8, u8), String> {
    let mut parts = input.split(':');
    let hour = parse_unit(parts.next(), 23, "hour")?;
    let minute = parse_unit(parts.next(), 59, "minute")?;
    let second = match parts.next() {
        None => 0,
        Some(value) => parse_unit(Some(value), 59, "second")?,
    };
    if parts.next().is_some() {
        return Err(PARSE_ERROR.to_string());
    }
    Ok((hour, minute, second))
}

fn parse_unit(part: Option<&str>, max: u8, _name: &str) -> Result<u8, String> {
    let Some(part) = part.filter(|part| !part.is_empty()) else {
        return Err(PARSE_ERROR.to_string());
    };
    let value = part.parse::<u8>().map_err(|_| PARSE_ERROR.to_string())?;
    if value > max {
        return Err(PARSE_ERROR.to_string());
    }
    Ok(value)
}

fn validate_date(year: u16, month: u8, day: u8) -> Result<(), String> {
    if year < 1970 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return Err(PARSE_ERROR.to_string());
    }
    Ok(())
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: u16) -> bool {
    let year = u32::from(year);
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn resolve_time_of_day(hour: u8, minute: u8, second: u8, oldest: SystemTime, newest: SystemTime) -> SystemTime {
    let query = seconds_of_day(hour, minute, second);
    let oldest_secs = unix_secs(oldest);
    let newest_secs = unix_secs(newest);
    let first_day = oldest_secs / 86_400;
    let last_day = newest_secs / 86_400;

    let mut latest_in_range = None;
    for day in first_day..=last_day {
        let candidate = day.saturating_mul(86_400).saturating_add(query);
        if candidate >= oldest_secs && candidate <= newest_secs {
            latest_in_range = Some(candidate);
        }
    }

    let target = latest_in_range.unwrap_or_else(|| {
        let first = first_day.saturating_mul(86_400).saturating_add(query);
        if first < oldest_secs { oldest_secs } else { newest_secs }
    });

    UNIX_EPOCH + Duration::from_secs(target)
}

fn seconds_of_day(hour: u8, minute: u8, second: u8) -> u64 {
    u64::from(hour) * 3_600 + u64::from(minute) * 60 + u64::from(second)
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or_default()
}

fn civil_to_system_time(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<SystemTime> {
    let days = days_from_civil(i32::from(year), u32::from(month), u32::from(day));
    let secs = days.checked_mul(86_400)?.checked_add(seconds_of_day(hour, minute, second) as i64)?;
    let secs = u64::try_from(secs).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// Days from the Unix epoch for a civil date. See Howard Hinnant, *chrono-Compatible Low-Level Date Algorithms*.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = (year - era * 400) as u32;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era) * 146_097 + i64::from(doe) - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_time_of_day_and_datetimes() {
        assert_eq!(TimeJump::parse("13:24").unwrap(), TimeJump::TimeOfDay { hour: 13, minute: 24, second: 0 });
        assert_eq!(TimeJump::parse("13:24:35").unwrap(), TimeJump::TimeOfDay { hour: 13, minute: 24, second: 35 });
        assert_eq!(TimeJump::parse("2026-09-06").unwrap(), TimeJump::Date { year: 2026, month: 9, day: 6 });
        assert_eq!(
            TimeJump::parse("2026-09-06 13:24:35").unwrap(),
            TimeJump::DateTime { year: 2026, month: 9, day: 6, hour: 13, minute: 24, second: 35 }
        );
        assert_eq!(
            TimeJump::parse("2026-09-06T13:24").unwrap(),
            TimeJump::DateTime { year: 2026, month: 9, day: 6, hour: 13, minute: 24, second: 0 }
        );
        assert!(TimeJump::parse("25:00").is_err());
        assert!(TimeJump::parse("2026-13-01").is_err());
        assert!(TimeJump::parse("not-a-time").is_err());
    }

    #[test]
    fn time_of_day_picks_the_latest_occurrence_in_range() {
        let oldest = UNIX_EPOCH + Duration::from_secs(10 * 3_600);
        let newest = UNIX_EPOCH + Duration::from_secs(15 * 3_600);
        let target = TimeJump::TimeOfDay { hour: 13, minute: 24, second: 0 }.resolve(oldest, newest);
        assert_eq!(target, UNIX_EPOCH + Duration::from_secs(13 * 3_600 + 24 * 60));
    }

    #[test]
    fn time_of_day_after_the_buffer_resolves_to_newest() {
        let oldest = UNIX_EPOCH + Duration::from_secs(10 * 3_600);
        let newest = UNIX_EPOCH + Duration::from_secs(12 * 3_600);
        let target = TimeJump::TimeOfDay { hour: 13, minute: 0, second: 0 }.resolve(oldest, newest);
        assert_eq!(target, newest);
    }

    #[test]
    fn unix_epoch_date_round_trips() {
        assert_eq!(civil_to_system_time(1970, 1, 1, 0, 0, 0), Some(UNIX_EPOCH));
        assert_eq!(
            civil_to_system_time(1970, 1, 2, 1, 2, 3),
            Some(UNIX_EPOCH + Duration::from_secs(86_400 + 3_600 + 120 + 3))
        );
    }
}
