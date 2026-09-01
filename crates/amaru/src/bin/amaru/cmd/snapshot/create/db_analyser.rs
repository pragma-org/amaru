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
    fs,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{Arc, Mutex},
    thread,
};

use amaru_kernel::Slot;
use amaru_progress_bar::ProgressBar;
use anyhow::anyhow;

const DB_ANALYSER_PROGRESS_REPORT_INTERVAL_SECS: f64 = 1.0;

pub(super) fn ensure_db_analyser_binary() -> anyhow::Result<String> {
    let binary = "db-analyser";

    let status = ProcessCommand::new(binary).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status();

    match status {
        Ok(_) => Ok(binary.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(anyhow!(
            "db-analyser was not found in $PATH. Add it to your $PATH (for example: export PATH=/opt/cardano-node/bin:$PATH)."
        )),
        Err(error) => {
            Err(anyhow!("failed to execute db-analyser preflight: {}. Ensure the binary is executable.", error))
        }
    }
}

pub(super) fn run_db_analyser(
    binary: &str,
    config_dir: &Path,
    db_dir: &Path,
    target_slot: Slot,
    analyse_from: Option<Slot>,
    with_progress: &Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
) -> anyhow::Result<()> {
    let config_dir = config_dir.canonicalize()?;
    let db_dir = db_dir.canonicalize()?;

    let mut command = ProcessCommand::new(binary);
    command.arg("--config").arg(config_dir.join("config.json")).arg("--db").arg(db_dir).arg("--in-mem");

    if let Some(analyse_from) = analyse_from {
        command.arg("--analyse-from").arg(analyse_from.to_string());
    }

    command.arg("--store-ledger").arg(target_slot.to_string());

    run_logged_command(command, "db-analyser", Some(DbAnalyserLogRelay::new(target_slot, analyse_from)), with_progress)
}

fn run_logged_command(
    mut command: ProcessCommand,
    step: &str,
    db_analyser_log_relay: Option<DbAnalyserLogRelay>,
    with_progress: &Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
) -> anyhow::Result<()> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("failed to capture child stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("failed to capture child stderr"))?;
    let db_analyser_log_relay = db_analyser_log_relay.map(|relay| Arc::new(Mutex::new(relay)));

    let tracked = db_analyser_log_relay.as_ref().map(|relay| TrackedProgress::new(relay, with_progress));
    let shared = tracked.as_ref().map(TrackedProgress::shared);

    let stdout_handle = spawn_log_relay(stdout, db_analyser_log_relay.clone(), shared.clone());
    let stderr_handle = spawn_log_relay(stderr, db_analyser_log_relay, shared);

    let status = child.wait()?;
    stdout_handle.join().map_err(|_| io::Error::other(format!("{step} stdout logger panicked")))??;
    stderr_handle.join().map_err(|_| io::Error::other(format!("{step} stderr logger panicked")))??;

    if let Some(t) = tracked {
        t.finish();
    }

    if !status.success() {
        anyhow::bail!("{step} failed with status {status}");
    }

    Ok(())
}

/// Manages the shared progress bar state and its animation ticker for a child process.
struct TrackedProgress {
    shared: Arc<Mutex<SharedProgress>>,
    ticker: Option<(thread::JoinHandle<()>, std::sync::mpsc::Sender<()>)>,
}

impl TrackedProgress {
    fn new(
        relay: &Arc<Mutex<DbAnalyserLogRelay>>,
        with_progress: &Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
    ) -> Self {
        let (restore_total, replay_total) =
            relay.lock().map(|r| (r.restore_total(), r.replay_total())).unwrap_or((0, 0));
        let shared = Arc::new(Mutex::new(SharedProgress {
            bar: None,
            restore_total,
            replay_total,
            factory: with_progress.clone(),
        }));
        let s = shared.clone();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            while stop_rx.try_recv().is_err() {
                if let Ok(s) = s.lock()
                    && let Some(pb) = s.bar.as_ref()
                {
                    pb.refresh();
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        Self { shared, ticker: Some((handle, stop_tx)) }
    }

    fn shared(&self) -> Arc<Mutex<SharedProgress>> {
        self.shared.clone()
    }

    fn finish(mut self) {
        if let Some((handle, stop_tx)) = self.ticker.take() {
            let _ = stop_tx.send(());
            let _ = handle.join();
        }
        if let Ok(mut s) = self.shared.lock()
            && let Some(pb) = s.bar.take()
        {
            pb.clear();
        }
    }
}

struct SharedProgress {
    bar: Option<Box<dyn ProgressBar + Send + Sync>>,
    restore_total: usize,
    replay_total: usize,
    factory: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
}

fn spawn_log_relay<R>(
    reader: R,
    db_analyser_log_relay: Option<Arc<Mutex<DbAnalyserLogRelay>>>,
    shared: Option<Arc<Mutex<SharedProgress>>>,
) -> thread::JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let line = line?;

            if let Some(db_analyser_log_relay) = db_analyser_log_relay.as_ref() {
                let action = db_analyser_log_relay
                    .lock()
                    .map_err(|_| io::Error::other("db-analyser progress relay poisoned"))?
                    .handle_line(&line);

                match action {
                    DbAnalyserLogAction::SwitchToReplay => {
                        if let Some(s) = shared.as_ref()
                            && let Ok(mut s) = s.lock()
                        {
                            if let Some(pb) = s.bar.take() {
                                pb.clear();
                            }
                            let factory = s.factory.clone();
                            let total = s.replay_total;
                            s.bar = Some(factory(
                                total,
                                "{spinner:.green} {per_sec}/s [{bar:40.green}] [{pos}/{len} slots] ({eta} remaining)",
                            ));
                        }
                        continue;
                    }
                    DbAnalyserLogAction::Progress { done } => {
                        if let Some(s) = shared.as_ref()
                            && let Ok(mut s) = s.lock()
                        {
                            if s.bar.is_none() {
                                let total = if s.restore_total > 0 { s.restore_total } else { s.replay_total };
                                let factory = s.factory.clone();
                                s.bar = Some(factory(
                                    total,
                                    "{spinner:.green} {bar:40.green} [{pos}/{len} slots] ({eta} remaining)",
                                ));
                            }
                            if let Some(pb) = s.bar.as_ref() {
                                if let Some(done) = done {
                                    pb.tick(done as usize);
                                } else {
                                    pb.refresh();
                                }
                            }
                        }
                        continue;
                    }
                    DbAnalyserLogAction::Suppress => continue,
                    DbAnalyserLogAction::PassThrough => {}
                }
            }
        }
        Ok(())
    })
}

#[derive(Debug)]
pub(super) struct DbAnalyserLogRelay {
    target_slot: Slot,
    start_slot: Slot,
    last_progress_report_elapsed_secs: Option<f64>,
    last_done: u64,
    in_restore_phase: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum DbAnalyserLogAction {
    PassThrough,
    Suppress,
    Progress { done: Option<u64> },
    SwitchToReplay,
}

impl DbAnalyserLogRelay {
    pub(super) fn new(target_slot: Slot, analyse_from: Option<Slot>) -> Self {
        let start_slot = analyse_from.unwrap_or_default();
        Self {
            target_slot,
            start_slot,
            last_progress_report_elapsed_secs: None,
            last_done: 0,
            in_restore_phase: start_slot > Slot::default(),
        }
    }

    pub(super) fn restore_total(&self) -> usize {
        self.start_slot.as_u64() as usize
    }

    pub(super) fn replay_total(&self) -> usize {
        self.target_slot.as_u64().saturating_sub(self.start_slot.as_u64()) as usize
    }

    pub(super) fn handle_line(&mut self, line: &str) -> DbAnalyserLogAction {
        if parse_db_analyser_started_line(line).is_some() {
            return DbAnalyserLogAction::Progress { done: None };
        }

        if let Some((elapsed_secs, current_slot)) = parse_db_analyser_progress_line(line) {
            if self.should_report_progress(elapsed_secs) {
                return self.progress_action(elapsed_secs, current_slot);
            }

            return DbAnalyserLogAction::Suppress;
        }

        if let Some((elapsed_secs, current_slot)) = parse_db_analyser_snapshot_stored_line(line) {
            return self.progress_action(elapsed_secs, current_slot);
        }
        if parse_db_analyser_done_line(line) {
            return DbAnalyserLogAction::Progress { done: None };
        }

        DbAnalyserLogAction::PassThrough
    }

    fn should_report_progress(&mut self, elapsed_secs: f64) -> bool {
        let should_report = self.last_progress_report_elapsed_secs.is_none_or(|last_elapsed_secs| {
            elapsed_secs - last_elapsed_secs >= DB_ANALYSER_PROGRESS_REPORT_INTERVAL_SECS
        });

        if should_report {
            self.last_progress_report_elapsed_secs = Some(elapsed_secs);
        }

        should_report
    }

    fn progress_action(&mut self, _elapsed_secs: f64, current_slot: Slot) -> DbAnalyserLogAction {
        if self.in_restore_phase && !self.is_restoring_resume_snapshot(current_slot) {
            self.in_restore_phase = false;
            self.last_done = self.start_slot.as_u64();
            return DbAnalyserLogAction::SwitchToReplay;
        }

        let current = current_slot.as_u64();
        let target = self.target_slot.as_u64();
        let capped = current.min(target);
        let delta = capped.saturating_sub(self.last_done);
        self.last_done = capped;

        DbAnalyserLogAction::Progress { done: Some(delta) }
    }

    fn is_restoring_resume_snapshot(&self, current_slot: Slot) -> bool {
        self.start_slot > Slot::default() && current_slot <= self.start_slot && self.start_slot < self.target_slot
    }
}

fn parse_db_analyser_elapsed_line(line: &str) -> Option<(f64, &str)> {
    let line = line.strip_prefix('[')?;
    let (elapsed_secs, rest) = line.split_once("s] ")?;
    Some((elapsed_secs.parse().ok()?, rest))
}

fn parse_db_analyser_started_line(line: &str) -> Option<f64> {
    let (elapsed_secs, rest) = parse_db_analyser_elapsed_line(line)?;
    rest.starts_with("Started StoreLedgerStateAt (SlotNo ").then_some(elapsed_secs)
}

pub(super) fn parse_db_analyser_progress_line(line: &str) -> Option<(f64, Slot)> {
    let (elapsed_secs, rest) = parse_db_analyser_elapsed_line(line)?;
    if !rest.starts_with("BlockNo ") {
        return None;
    }
    let slot_fragment = rest.split_once("SlotNo ")?.1;
    let slot = slot_fragment.split_whitespace().next()?.parse().ok()?;
    Some((elapsed_secs, slot))
}

fn parse_db_analyser_snapshot_stored_line(line: &str) -> Option<(f64, Slot)> {
    let (elapsed_secs, rest) = parse_db_analyser_elapsed_line(line)?;
    let slot = rest.strip_prefix("Snapshot stored at SlotNo ")?.split_whitespace().next()?.parse().ok()?;
    Some((elapsed_secs, slot))
}

fn parse_db_analyser_done_line(line: &str) -> bool {
    parse_db_analyser_elapsed_line(line).is_some_and(|(_, rest)| rest == "Done")
}

pub(super) fn exact_snapshot_dir(ledger_snapshot_dir: &Path, slot: Slot) -> Option<PathBuf> {
    let path = ledger_snapshot_dir.join(format!("{slot}_db-analyser"));
    path.is_dir().then_some(path)
}

pub(super) fn select_analyse_from_slot(
    ledger_snapshot_dir: &Path,
    target_slot: Slot,
    previous_snapshot_slot: Option<Slot>,
) -> anyhow::Result<Option<Slot>> {
    let Some(previous_snapshot_slot) = previous_snapshot_slot else {
        return Ok(latest_snapshot_slot_at_or_before(ledger_snapshot_dir, target_slot)?);
    };

    if previous_snapshot_slot > target_slot {
        anyhow::bail!(
            "resume snapshot slot {} is greater than the target slot {}",
            previous_snapshot_slot,
            target_slot
        );
    }

    let snapshot_dir = ledger_snapshot_dir.join(format!("{previous_snapshot_slot}_db-analyser"));
    if !snapshot_dir.is_dir() {
        anyhow::bail!(
            "resume snapshot slot {} requires an existing snapshot directory at {}",
            previous_snapshot_slot,
            snapshot_dir.display()
        );
    }

    Ok(Some(previous_snapshot_slot))
}

pub(super) fn latest_snapshot_slot_at_or_before(
    ledger_snapshot_dir: &Path,
    target_slot: Slot,
) -> Result<Option<Slot>, io::Error> {
    if !ledger_snapshot_dir.try_exists()? {
        return Ok(None);
    }

    let mut best: Option<Slot> = None;
    for entry in fs::read_dir(ledger_snapshot_dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(slot) = parse_snapshot_slot_dir_name(&name) else {
            continue;
        };
        if slot <= target_slot {
            best = Some(best.map_or(slot, |current| current.max(slot)));
        }
    }

    Ok(best)
}

pub(super) fn parse_snapshot_slot_dir_name(name: &str) -> Option<Slot> {
    name.strip_suffix("_db-analyser")?.parse().ok()
}
