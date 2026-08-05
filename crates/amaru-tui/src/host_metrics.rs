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

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command;
use std::{
    io,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use sysinfo::{DiskRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System, get_current_pid};

use crate::events::{HostSample, Message};

const POLL_DELAY: Duration = Duration::from_secs(5);

pub struct Sampler {
    shutdown_tx: Sender<()>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl Sampler {
    pub fn spawn(tx: SyncSender<Message>) -> io::Result<Self> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("amaru-tui-host-metrics".into())
            .spawn(move || run(tx, shutdown_rx))
            .map_err(|err| io::Error::other(format!("failed to spawn host metrics thread: {err}")))?;

        Ok(Self { shutdown_tx, join: Some(join) })
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> io::Result<()> {
        let _ = self.shutdown_tx.send(());
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| io::Error::other("host metrics thread panicked"))?
        } else {
            Ok(())
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

fn run(tx: SyncSender<Message>, shutdown_rx: Receiver<()>) -> io::Result<()> {
    let mut sys =
        System::new_with_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()));
    let mut disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_io_usage());
    let own_pid = get_current_pid().map_err(|err| io::Error::other(format!("failed to resolve own pid: {err}")))?;
    let mut last_sampled_at = Instant::now();

    emit_sample(&mut sys, &mut disks, &tx, own_pid, Duration::ZERO, last_sampled_at);

    loop {
        match shutdown_rx.recv_timeout(POLL_DELAY) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        let interval = now.duration_since(last_sampled_at);
        last_sampled_at = now;

        emit_sample(&mut sys, &mut disks, &tx, own_pid, interval, now);
    }
}

fn emit_sample(
    sys: &mut System,
    disks: &mut Disks,
    tx: &SyncSender<Message>,
    own_pid: sysinfo::Pid,
    interval: Duration,
    at: Instant,
) {
    sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    disks.refresh_specifics(false, DiskRefreshKind::nothing().with_io_usage());
    let process_memory = sample_process_memory(own_pid);
    let (host_live_read_bytes, host_live_write_bytes) =
        disks.iter().fold((0u64, 0u64), |(read_total, write_total), disk| {
            let usage = disk.usage();
            (read_total.saturating_add(usage.read_bytes), write_total.saturating_add(usage.written_bytes))
        });

    let _ = tx.try_send(Message::HostSample(HostSample {
        at,
        interval,
        process_memory_bytes: process_memory,
        memory_used_bytes: sys.used_memory(),
        memory_total_bytes: sys.total_memory(),
        host_live_read_bytes,
        host_live_write_bytes,
    }));
}

#[cfg(target_os = "macos")]
fn sample_process_memory(pid: sysinfo::Pid) -> Option<u64> {
    let output =
        Command::new("top").args(["-l", "1", "-pid", &pid.as_u32().to_string(), "-stats", "pid,mem"]).output().ok()?;

    if !output.status.success() {
        return None;
    }

    parse_top_process_memory(&String::from_utf8_lossy(&output.stdout), pid.as_u32())
}

#[cfg(target_os = "linux")]
fn sample_process_memory(pid: sysinfo::Pid) -> Option<u64> {
    let output = Command::new("top").args(["-b", "-n", "1", "-p", &pid.as_u32().to_string()]).output().ok()?;

    if !output.status.success() {
        return None;
    }

    parse_linux_top_process_memory(&String::from_utf8_lossy(&output.stdout), pid.as_u32())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_process_memory(_pid: sysinfo::Pid) -> Option<u64> {
    None
}

#[cfg(any(target_os = "macos", test))]
fn parse_top_process_memory(output: &str, pid: u32) -> Option<u64> {
    output.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        let line_pid = fields.next()?.parse::<u32>().ok()?;
        if line_pid != pid {
            return None;
        }

        parse_top_bytes(fields.next()?, PlainUnit::Bytes)
    })
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_top_process_memory(output: &str, pid: u32) -> Option<u64> {
    output.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        let line_pid = fields.next()?.parse::<u32>().ok()?;
        if line_pid != pid {
            return None;
        }

        for _ in 0..4 {
            fields.next()?;
        }

        parse_top_bytes(fields.next()?, PlainUnit::KiB)
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
#[allow(dead_code)]
enum PlainUnit {
    Bytes,
    KiB,
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn parse_top_bytes(value: &str, plain_unit: PlainUnit) -> Option<u64> {
    let value = value.trim_end_matches('+');
    let suffix = value.chars().last()?;

    let multiplier = match suffix {
        'K' => 1_024f64,
        'M' => 1_024f64 * 1_024f64,
        'G' => 1_024f64 * 1_024f64 * 1_024f64,
        'T' => 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64,
        'P' => 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64,
        'k' => 1_024f64,
        'm' => 1_024f64 * 1_024f64,
        'g' => 1_024f64 * 1_024f64 * 1_024f64,
        't' => 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64,
        'p' => 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64 * 1_024f64,
        '0'..='9' => {
            let amount = value.parse::<u64>().ok()?;
            return Some(match plain_unit {
                PlainUnit::Bytes => amount,
                PlainUnit::KiB => amount.saturating_mul(1_024),
            });
        }
        _ => return None,
    };

    let amount = value[..value.len() - 1].parse::<f64>().ok()?;
    Some((amount * multiplier).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_memory_suffixes() {
        assert_eq!(parse_top_bytes("1201K", PlainUnit::Bytes), Some(1_229_824));
        assert_eq!(parse_top_bytes("1.5M", PlainUnit::Bytes), Some(1_572_864));
        assert_eq!(parse_top_bytes("2.0G", PlainUnit::Bytes), Some(2_147_483_648));
        assert_eq!(parse_top_bytes("42", PlainUnit::Bytes), Some(42));
        assert_eq!(parse_top_bytes("42", PlainUnit::KiB), Some(43_008));
        assert_eq!(parse_top_bytes("1.5g", PlainUnit::KiB), Some(1_610_612_736));
    }

    #[test]
    fn parses_top_process_sample_for_expected_pid() {
        let output = "\
Processes: 1 total\n\
PID    MEM\n\
73194  1.5G\n";

        assert_eq!(parse_top_process_memory(output, 73_194), Some(1_610_612_736));
    }

    #[test]
    fn parses_linux_top_process_sample_for_expected_pid() {
        let output = "\
top - 12:00:00 up 1 day,  1 user,  load average: 0.00, 0.00, 0.00\n\
Tasks:   1 total,   1 running,   0 sleeping,   0 stopped,   0 zombie\n\
%Cpu(s):  0.0 us,  0.0 sy,  0.0 ni,100.0 id,  0.0 wa,  0.0 hi,  0.0 si,  0.0 st \n\
MiB Mem :   1024.0 total,    256.0 free,    512.0 used,    256.0 buff/cache\n\
\n\
    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND\n\
  73194 user      20   0 1234567 654321  12345 S   0.0   0.1   0:00.01 amaru\n";

        assert_eq!(parse_linux_top_process_memory(output, 73_194), Some(670_024_704));
    }
}
