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

use tracing::{info, warn};

use super::{config::DbAnalyserBuildConfig, repo_root};

const DB_ANALYSER_PROGRESS_REPORT_INTERVAL_SECS: f64 = 30.0;

pub(super) fn ensure_db_analyser_image(
    db_analyser_build_config: &DbAnalyserBuildConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let image_ref =
        format!("{}:{}", db_analyser_build_config.image, sanitize_image_tag(&db_analyser_build_config.git_ref));
    let status = ProcessCommand::new("docker").args(["image", "inspect", image_ref.as_str()]).status()?;
    if status.success() {
        info!(image = %image_ref, "reusing cached db-analyser image");
        return Ok(image_ref);
    }

    info!(image = %image_ref, git_ref = %db_analyser_build_config.git_ref, "building db-analyser Docker image");
    let dockerfile = repo_root().join("docker").join("Dockerfile.db-analyser");

    let mut command = ProcessCommand::new("docker");
    command
        .arg("build")
        .arg("-t")
        .arg(&image_ref)
        .arg("-f")
        .arg(&dockerfile)
        .arg("--build-arg")
        .arg(format!("OUROBOROS_CONSENSUS_REF={}", db_analyser_build_config.git_ref))
        .arg(repo_root());
    run_logged_command(command, "docker-build", None)?;

    Ok(image_ref)
}

pub(super) fn sanitize_image_tag(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '.' | '-' => character,
            _ => '-',
        })
        .collect()
}

pub(super) fn run_db_analyser(
    image: &str,
    config_dir: &Path,
    db_dir: &Path,
    target_slot: u64,
    analyse_from: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = config_dir.canonicalize()?;
    let db_dir = db_dir.canonicalize()?;

    let mut command = ProcessCommand::new("docker");
    command
        .arg("run")
        .arg("--rm")
        .arg("--security-opt")
        .arg("seccomp=unconfined")
        .arg("-v")
        .arg(format!("{}:/config:ro", config_dir.display()))
        .arg("-v")
        .arg(format!("{}:/db", db_dir.display()))
        .arg(image)
        .arg("--config")
        .arg("/config/config.json")
        .arg("--db")
        .arg("/db")
        .arg("--in-mem");

    if let Some(analyse_from) = analyse_from {
        command.arg("--analyse-from").arg(analyse_from.to_string());
    }

    command.arg("--store-ledger").arg(target_slot.to_string());

    run_logged_command(command, "db-analyser", Some(DbAnalyserLogRelay::new(target_slot, analyse_from)))
}

fn run_logged_command(
    mut command: ProcessCommand,
    step: &str,
    db_analyser_log_relay: Option<DbAnalyserLogRelay>,
) -> Result<(), Box<dyn std::error::Error>> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or("failed to capture child stdout")?;
    let stderr = child.stderr.take().ok_or("failed to capture child stderr")?;
    let db_analyser_log_relay = db_analyser_log_relay.map(|relay| Arc::new(Mutex::new(relay)));

    let stdout_handle = spawn_log_relay(stdout, step.to_string(), false, db_analyser_log_relay.clone());
    let stderr_handle = spawn_log_relay(stderr, step.to_string(), true, db_analyser_log_relay);

    let status = child.wait()?;
    stdout_handle.join().map_err(|_| io::Error::other(format!("{step} stdout logger panicked")))??;
    stderr_handle.join().map_err(|_| io::Error::other(format!("{step} stderr logger panicked")))??;

    if !status.success() {
        return Err(format!("{step} failed with status {status}").into());
    }

    Ok(())
}

fn spawn_log_relay<R>(
    reader: R,
    step: String,
    is_stderr: bool,
    db_analyser_log_relay: Option<Arc<Mutex<DbAnalyserLogRelay>>>,
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
                    DbAnalyserLogAction::Report(message) => {
                        info!(step = %step, message = %message, "external command progress");
                        continue;
                    }
                    DbAnalyserLogAction::Suppress => continue,
                    DbAnalyserLogAction::PassThrough => {}
                }
            }

            if is_stderr {
                warn!(step = %step, line = %line, "external command output");
            } else {
                info!(step = %step, line = %line, "external command output");
            }
        }
        Ok(())
    })
}

#[derive(Debug)]
pub(super) struct DbAnalyserLogRelay {
    target_slot: u64,
    start_slot: u64,
    last_progress_report_elapsed_secs: Option<f64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DbAnalyserLogAction {
    PassThrough,
    Suppress,
    Report(String),
}

impl DbAnalyserLogRelay {
    pub(super) fn new(target_slot: u64, analyse_from: Option<u64>) -> Self {
        Self { target_slot, start_slot: analyse_from.unwrap_or(0), last_progress_report_elapsed_secs: None }
    }

    pub(super) fn handle_line(&mut self, line: &str) -> DbAnalyserLogAction {
        if parse_db_analyser_started_line(line).is_some() {
            return DbAnalyserLogAction::Report(self.started_message());
        }

        if let Some((elapsed_secs, current_slot)) = parse_db_analyser_progress_line(line) {
            if self.should_report_progress(elapsed_secs) {
                return DbAnalyserLogAction::Report(self.progress_message(elapsed_secs, current_slot));
            }

            return DbAnalyserLogAction::Suppress;
        }

        if let Some((elapsed_secs, current_slot)) = parse_db_analyser_snapshot_stored_line(line) {
            return DbAnalyserLogAction::Report(self.progress_message(elapsed_secs, current_slot));
        }

        if let Some(elapsed_secs) = parse_db_analyser_done_line(line) {
            return DbAnalyserLogAction::Report(format!("db-analyser finished in {}", format_seconds(elapsed_secs)));
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

    fn started_message(&self) -> String {
        if self.start_slot == 0 {
            format!("db-analyser started: replaying to slot {}", self.target_slot)
        } else {
            format!(
                "db-analyser started: resuming from stored ledger snapshot at slot {} and replaying to slot {}",
                self.start_slot, self.target_slot
            )
        }
    }

    fn progress_message(&self, elapsed_secs: f64, current_slot: u64) -> String {
        if self.is_restoring_resume_snapshot(current_slot) {
            return format!(
                "db-analyser resume: still restoring stored ledger snapshot at slot {} before replaying to slot {} (elapsed {})",
                self.start_slot,
                self.target_slot,
                format_seconds(elapsed_secs),
            );
        }

        let capped_slot = current_slot.clamp(self.start_slot, self.target_slot);
        let done_slots = capped_slot.saturating_sub(self.start_slot);
        let total_slots = self.target_slot.saturating_sub(self.start_slot).max(1);
        let progress = done_slots as f64 / total_slots as f64;
        let eta_secs =
            if progress > 0.0 && progress < 1.0 { elapsed_secs * ((1.0 - progress) / progress) } else { 0.0 };

        format!(
            "db-analyser progress: {:.1}% (slot {}/{}, elapsed {}, eta {})",
            progress * 100.0,
            capped_slot,
            self.target_slot,
            format_seconds(elapsed_secs),
            format_seconds(eta_secs),
        )
    }

    fn is_restoring_resume_snapshot(&self, current_slot: u64) -> bool {
        self.start_slot > 0 && current_slot <= self.start_slot && self.start_slot < self.target_slot
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

pub(super) fn parse_db_analyser_progress_line(line: &str) -> Option<(f64, u64)> {
    let (elapsed_secs, rest) = parse_db_analyser_elapsed_line(line)?;
    if !rest.starts_with("BlockNo ") {
        return None;
    }

    let slot_fragment = rest.split_once("SlotNo ")?.1;
    let slot = slot_fragment.split_whitespace().next()?.parse().ok()?;
    Some((elapsed_secs, slot))
}

fn parse_db_analyser_snapshot_stored_line(line: &str) -> Option<(f64, u64)> {
    let (elapsed_secs, rest) = parse_db_analyser_elapsed_line(line)?;
    let slot = rest.strip_prefix("Snapshot stored at SlotNo ")?.split_whitespace().next()?.parse().ok()?;
    Some((elapsed_secs, slot))
}

fn parse_db_analyser_done_line(line: &str) -> Option<f64> {
    let (elapsed_secs, rest) = parse_db_analyser_elapsed_line(line)?;
    (rest == "Done").then_some(elapsed_secs)
}

fn format_seconds(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

pub(super) fn exact_snapshot_dir(ledger_snapshot_dir: &Path, slot: u64) -> Option<PathBuf> {
    let path = ledger_snapshot_dir.join(format!("{slot}_db-analyser"));
    path.is_dir().then_some(path)
}

pub(super) fn select_analyse_from_slot(
    ledger_snapshot_dir: &Path,
    target_slot: u64,
    previous_snapshot_slot: Option<u64>,
) -> Result<Option<u64>, Box<dyn std::error::Error>> {
    let Some(previous_snapshot_slot) = previous_snapshot_slot else {
        return Ok(latest_snapshot_slot_at_or_before(ledger_snapshot_dir, target_slot)?);
    };

    if previous_snapshot_slot > target_slot {
        return Err(format!(
            "resume snapshot slot {} is greater than the target slot {}",
            previous_snapshot_slot, target_slot
        )
        .into());
    }

    let snapshot_dir = ledger_snapshot_dir.join(format!("{previous_snapshot_slot}_db-analyser"));
    if !snapshot_dir.is_dir() {
        return Err(format!(
            "resume snapshot slot {} requires an existing snapshot directory at {}",
            previous_snapshot_slot,
            snapshot_dir.display()
        )
        .into());
    }

    Ok(Some(previous_snapshot_slot))
}

pub(super) fn latest_snapshot_slot_at_or_before(
    ledger_snapshot_dir: &Path,
    target_slot: u64,
) -> Result<Option<u64>, io::Error> {
    if !ledger_snapshot_dir.try_exists()? {
        return Ok(None);
    }

    let mut best: Option<u64> = None;
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

pub(super) fn parse_snapshot_slot_dir_name(name: &str) -> Option<u64> {
    name.strip_suffix("_db-analyser")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{DbAnalyserLogAction, DbAnalyserLogRelay};

    #[test]
    fn started_message_explains_resume_source() {
        let mut relay = DbAnalyserLogRelay::new(134_524_753, Some(134_092_758));

        assert_eq!(
            relay.handle_line("[0.0s] Started StoreLedgerStateAt (SlotNo 134524753)"),
            DbAnalyserLogAction::Report(
                "db-analyser started: resuming from stored ledger snapshot at slot 134092758 and replaying to slot 134524753"
                    .to_owned()
            )
        );
    }

    #[test]
    fn progress_message_describes_resume_restore_before_replay() {
        let mut relay = DbAnalyserLogRelay::new(134_524_753, Some(134_092_758));

        assert_eq!(
            relay.handle_line("[32.0s] BlockNo 42 SlotNo 134092758"),
            DbAnalyserLogAction::Report(
                "db-analyser resume: still restoring stored ledger snapshot at slot 134092758 before replaying to slot 134524753 (elapsed 32s)"
                    .to_owned()
            )
        );
    }

    #[test]
    fn progress_message_switches_to_percentage_after_resume_slot() {
        let mut relay = DbAnalyserLogRelay::new(134_524_753, Some(134_092_758));

        assert_eq!(
            relay.handle_line("[32.0s] BlockNo 42 SlotNo 134100000"),
            DbAnalyserLogAction::Report(
                "db-analyser progress: 1.7% (slot 134100000/134524753, elapsed 32s, eta 31m 17s)".to_owned()
            )
        );
    }
}
