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

use std::{
    io,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

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
    let mut last_sampled_at = Instant::now();

    emit_sample(&mut sys, &tx, Duration::ZERO, last_sampled_at);

    loop {
        match shutdown_rx.recv_timeout(POLL_DELAY) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        let interval = now.duration_since(last_sampled_at);
        last_sampled_at = now;

        emit_sample(&mut sys, &tx, interval, now);
    }
}

fn emit_sample(sys: &mut System, tx: &SyncSender<Message>, interval: Duration, at: Instant) {
    sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_disk_usage());

    let (processes_live_read_bytes, processes_live_write_bytes) =
        sys.processes().values().fold((0u64, 0u64), |(read_total, write_total), process| {
            let disk_usage = process.disk_usage();
            (read_total.saturating_add(disk_usage.read_bytes), write_total.saturating_add(disk_usage.written_bytes))
        });

    let _ = tx.try_send(Message::HostSample(HostSample {
        at,
        interval,
        memory_used_bytes: sys.used_memory(),
        memory_total_bytes: sys.total_memory(),
        processes_live_read_bytes,
        processes_live_write_bytes,
    }));
}
