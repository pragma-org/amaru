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

#[cfg(unix)]
use std::process::Command;

/// Read the current process's cumulative block-I/O delay in centiseconds.
///
/// Linux exposes this as field 42 (`delayacct_blkio_ticks`) in
/// `/proc/self/stat`. Other Unix platforms do not provide the equivalent
/// Linux process statistic and report zero, matching `cardano-node`.
///
/// `cardano-node` delegates resource collection to this Linux sampler:
/// <https://github.com/IntersectMBO/hermod-tracing/blob/d322eb06fa2b351c0943ff2b80a94bf5ed954cd0/hermod-trace-resources/src/Hermod/Tracing/Resources/Linux.hs#L103-L120>
pub fn sample_process_block_io_ticks() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|stat| parse_process_block_io_ticks(&stat))
            .unwrap_or(0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_process_block_io_ticks(stat: &str) -> Option<u64> {
    const PROCESS_STATE_FIELD: usize = 3;
    const BLOCK_IO_TICKS_FIELD: usize = 42;

    let (_, fields_after_command) = stat.rsplit_once(") ")?;
    fields_after_command
        .split_whitespace()
        .nth(BLOCK_IO_TICKS_FIELD - PROCESS_STATE_FIELD)
        .and_then(|value| value.parse().ok())
}

#[cfg(unix)]
pub fn sample_process_memory(pid: u32) -> Option<u64> {
    let output = if cfg!(target_os = "macos") {
        Command::new("top").args(["-l", "1", "-pid", &pid.to_string(), "-stats", "pid,mem"]).output().ok()?
    } else {
        Command::new("top").args(["-b", "-n", "1", "-p", &pid.to_string()]).output().ok()?
    };

    if !output.status.success() {
        return None;
    }

    parse_top_mem(&String::from_utf8_lossy(&output.stdout), pid)
}

#[cfg(unix)]
fn parse_top_mem(output: &str, pid: u32) -> Option<u64> {
    output.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()?.parse::<u32>().ok()? != pid {
            return None;
        }

        if cfg!(target_os = "linux") {
            for _ in 0..4 {
                fields.next()?;
            }
        }

        let multiplier = if cfg!(target_os = "linux") { 1024 } else { 1 };

        parse_value_with_unit(fields.next()?, multiplier)
    })
}

#[cfg(unix)]
fn parse_value_with_unit(value: &str, plain_multiplier: u64) -> Option<u64> {
    let value = value.trim_end_matches('+');
    let suffix = value.chars().last()?;

    let multiplier = match suffix {
        'K' | 'k' => 1_024f64,
        'M' | 'm' => 1_024f64 * 1_024f64,
        'G' | 'g' => 1_024f64 * 1_024f64 * 1_024f64,
        'T' | 't' => 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64,
        'P' | 'p' => 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64,
        '0'..='9' => {
            let amount = value.parse::<u64>().ok()?;
            return Some(amount.saturating_mul(plain_multiplier));
        }
        _ => return None,
    };

    let amount = value[..value.len() - 1].parse::<f64>().ok()?;

    Some((amount * multiplier).round() as u64)
}

#[cfg(all(test, unix))]
mod tests {
    use test_case::test_case;

    use super::{parse_top_mem, parse_value_with_unit};

    #[test]
    fn parses_process_block_io_ticks() {
        let mut fields = vec!["S".to_string()];
        fields.extend((4..42).map(|field| field.to_string()));
        fields.push("987".to_string());
        let stat = format!("123 (amaru (worker)) {}", fields.join(" "));

        assert_eq!(super::parse_process_block_io_ticks(&stat), Some(987));
    }

    #[test]
    fn rejects_truncated_process_stat() {
        assert_eq!(super::parse_process_block_io_ticks("123 (amaru) S 1 2 3"), None);
    }

    #[test_case("1201K", 1 => Some(1_229_824))]
    #[test_case("1.5M", 1 => Some(1_572_864))]
    #[test_case("2.0G", 1 => Some(2_147_483_648))]
    #[test_case("42", 1 => Some(42))]
    #[test_case("42", 1024 => Some(43_008))]
    #[test_case("1.5g", 1 => Some(1_610_612_736))]
    #[test_case("150M+", 1 => Some(157_286_400))]
    fn parses_top_memory_suffixes(value: &str, plain_multiplier: u64) -> Option<u64> {
        parse_value_with_unit(value, plain_multiplier)
    }

    #[test]
    fn parse_top_process_for_memory() {
        let output = if cfg!(target_os = "linux") {
            [
                "top - 12:00:00 up 1 day,  1 user,  load average: 0.00, 0.00, 0.00",
                "Tasks:   1 total,   1 running,   0 sleeping,   0 stopped,   0 zombie",
                "%Cpu(s):  0.0 us,  0.0 sy,  0.0 ni,100.0 id,  0.0 wa,  0.0 hi,  0.0 si,  0.0 st ",
                "MiB Mem :   1024.0 total,    256.0 free,    512.0 used,    256.0 buff/cache",
                "",
                "    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND",
                "  73194 user      20   0 1234567 654321  12345 S   0.0   0.1   0:00.01 amaru",
                "",
            ]
            .join("\n")
        } else {
            ["Processes: 1 total", "PID    MEM", "73194  654321K", ""].join("\n")
        };

        assert_eq!(parse_top_mem(&output, 73_194), Some(670_024_704));
    }
}
