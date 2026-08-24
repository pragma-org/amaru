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

#![cfg(unix)]

use std::{process::Output, time::Duration};

use assert_cmd::{Command, cargo::cargo_bin};
use tempfile::TempDir;

fn run_under_low_fd_limit(color: &str) -> anyhow::Result<Output> {
    let root = TempDir::new()?;
    let ledger_dir = root.path().join("ledger.preprod.db");
    std::fs::create_dir(&ledger_dir)?;
    let chain_dir = root.path().join("chain.preprod.db");
    std::fs::create_dir(&chain_dir)?;
    let amaru = cargo_bin("amaru");

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("ulimit -n 256; exec \"$@\"")
        .arg("sh")
        .arg(amaru)
        .arg("--color")
        .arg(color)
        .arg("run")
        .arg("--peer-address")
        .arg("127.0.0.1:65532")
        .arg("--ledger-dir")
        .arg(&ledger_dir)
        .arg("--chain-dir")
        .arg(&chain_dir)
        .env("AMARU_NETWORK", "preprod")
        .timeout(Duration::from_secs(15));

    Ok(command.output()?)
}

fn combined_output(output: &Output) -> Vec<u8> {
    let mut bytes = output.stdout.clone();
    bytes.extend_from_slice(&output.stderr);
    bytes
}

fn contains_ansi_escape(bytes: &[u8]) -> bool {
    bytes.windows(2).any(|window| window == b"\x1b[")
}

#[test]
fn explains_fd_limit_is_too_low() -> anyhow::Result<()> {
    let output = run_under_low_fd_limit("never")?;
    let rendered = combined_output(&output);
    let rendered = String::from_utf8_lossy(&rendered);

    assert!(!output.status.success());
    assert!(rendered.contains("Increase the limit for open files before starting Amaru"), "got {}", rendered);

    Ok(())
}

#[test]
fn no_color_when_color_is_never() -> anyhow::Result<()> {
    let output = run_under_low_fd_limit("never")?;
    let rendered = combined_output(&output);

    assert!(!output.status.success());
    assert!(
        !contains_ansi_escape(&rendered),
        "found ANSI escape codes in output:\n{}",
        String::from_utf8_lossy(&rendered)
    );

    Ok(())
}

#[test]
fn color_when_color_is_always() -> anyhow::Result<()> {
    let output = run_under_low_fd_limit("always")?;
    let rendered = combined_output(&output);

    assert!(!output.status.success());
    assert!(
        contains_ansi_escape(&rendered),
        "expected ANSI escape codes in output but found none:\n{}",
        String::from_utf8_lossy(&rendered)
    );

    Ok(())
}

fn amaru_help(args: &[&str]) -> anyhow::Result<String> {
    let amaru = cargo_bin("amaru");
    let mut command = Command::new(amaru);
    for arg in args {
        command.arg(arg);
    }
    command.arg("--help");
    let output = command.output()?;
    assert!(output.status.success(), "amaru {} --help failed", args.join(" "));
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
fn top_level_help_shows_visible_commands() -> anyhow::Result<()> {
    let help = amaru_help(&[])?;
    assert!(help.contains("node"), "top-level help should show 'node'");
    assert!(help.contains("snapshot"), "top-level help should show 'snapshot'");
    assert!(help.contains("mithril"), "top-level help should show 'mithril'");
    assert!(!help.contains("dev"), "top-level help should NOT show hidden 'dev'");
    assert!(!help.contains("dump-chain-db"), "top-level help should NOT show legacy commands");
    assert!(!help.contains("remove-validation-status"), "top-level help should NOT show legacy commands");
    Ok(())
}

#[test]
fn mithril_sync_help_shows_all_options() -> anyhow::Result<()> {
    let help = amaru_help(&["mithril", "sync"])?;
    assert!(help.contains("--network"), "mithril sync should accept --network");
    assert!(help.contains("--ledger-dir"), "mithril sync should accept --ledger-dir");
    assert!(help.contains("--chain-dir"), "mithril sync should accept --chain-dir");
    assert!(help.contains("--snapshots-dir"), "mithril sync should accept --snapshots-dir");
    assert!(help.contains("--ingest-until-slot"), "mithril sync should accept --ingest-until-slot");
    assert!(help.contains("--ingest-maximum-blocks"), "mithril sync should accept --ingest-maximum-blocks");
    Ok(())
}

#[test]
fn removed_dev_ledger_mithril_commands_are_rejected() -> anyhow::Result<()> {
    let amaru = cargo_bin("amaru");
    for command in ["mithril", "sync"] {
        let output = Command::new(&amaru).args(["dev", "ledger", command, "--help"]).output()?;
        assert!(!output.status.success(), "removed 'dev ledger {command}' command should be rejected");
    }
    Ok(())
}

#[test]
fn node_help_shows_subcommands() -> anyhow::Result<()> {
    let help = amaru_help(&["node"])?;
    assert!(help.contains("run"), "node help should show 'run'");
    assert!(help.contains("bootstrap"), "node help should show 'bootstrap'");
    assert!(help.contains("rollback"), "node help should show 'rollback'");
    assert!(help.contains("rm"), "node help should show 'rm'");
    Ok(())
}

#[test]
fn node_rollback_help_shows_targets() -> anyhow::Result<()> {
    let help = amaru_help(&["node", "rollback"])?;
    assert!(help.contains("--immutable-tip"), "rollback should accept --immutable-tip");
    assert!(help.contains("--epoch"), "rollback should accept --epoch");
    assert!(help.contains("--network"), "rollback should accept --network");
    assert!(help.contains("--chain-dir"), "rollback should accept --chain-dir");
    assert!(help.contains("--ledger-dir"), "rollback should accept --ledger-dir");
    Ok(())
}

#[test]
fn snapshot_help_shows_subcommands() -> anyhow::Result<()> {
    let help = amaru_help(&["snapshot"])?;
    assert!(help.contains("create"), "snapshot help should show 'create'");
    Ok(())
}

#[test]
fn dev_help_shows_subcommands() -> anyhow::Result<()> {
    let help = amaru_help(&["dev"])?;
    assert!(help.contains("chain"), "dev help should show 'chain'");
    assert!(help.contains("ledger"), "dev help should show 'ledger'");
    assert!(help.contains("traces"), "dev help should show 'traces'");
    Ok(())
}

#[test]
fn dev_chain_help_shows_subcommands() -> anyhow::Result<()> {
    let help = amaru_help(&["dev", "chain"])?;
    assert!(help.contains("dump"), "dev chain help should show 'dump'");
    assert!(help.contains("clear-invalid"), "dev chain help should show 'clear-invalid'");
    assert!(help.contains("fetch"), "dev chain help should show 'fetch'");
    assert!(help.contains("migrate"), "dev chain help should show 'migrate'");
    assert!(help.contains("remove"), "dev chain help should show 'remove'");
    Ok(())
}

#[test]
fn dev_traces_help_shows_subcommands() -> anyhow::Result<()> {
    let help = amaru_help(&["dev", "traces"])?;
    assert!(help.contains("dump"), "dev traces help should show 'dump'");
    Ok(())
}

#[test]
fn legacy_run_alias_works() -> anyhow::Result<()> {
    let help = amaru_help(&["run"])?;
    assert!(help.contains("--network"), "legacy 'run' should accept --network");
    assert!(help.contains("--listen-address"), "legacy 'run' should accept --listen-address");
    Ok(())
}

#[test]
fn legacy_bootstrap_alias_works() -> anyhow::Result<()> {
    let help = amaru_help(&["bootstrap"])?;
    assert!(help.contains("--network"), "legacy 'bootstrap' should accept --network");
    Ok(())
}

#[test]
fn legacy_reset_to_epoch_alias_works() -> anyhow::Result<()> {
    let help = amaru_help(&["reset-to-epoch"])?;
    assert!(help.contains("--network"), "legacy 'reset-to-epoch' should accept --network");
    Ok(())
}

#[test]
fn legacy_create_snapshots_alias_works() -> anyhow::Result<()> {
    let help = amaru_help(&["create-snapshots"])?;
    assert!(help.contains("--network"), "legacy 'create-snapshots' should accept --network");
    Ok(())
}

#[test]
fn legacy_dump_chain_db_alias_works() -> anyhow::Result<()> {
    let help = amaru_help(&["dump-chain-db"])?;
    assert!(help.contains("--network"), "legacy 'dump-chain-db' should accept --network");
    Ok(())
}

#[test]
fn legacy_migrate_chain_db_alias_works() -> anyhow::Result<()> {
    let help = amaru_help(&["migrate-chain-db"])?;
    assert!(help.contains("--network"), "legacy 'migrate-chain-db' should accept --network");
    Ok(())
}

#[test]
fn node_run_help_matches_legacy_run() -> anyhow::Result<()> {
    let node_run_help = amaru_help(&["node", "run"])?;
    assert!(node_run_help.contains("--network"), "node run should accept --network");
    assert!(node_run_help.contains("--listen-address"), "node run should accept --listen-address");
    assert!(node_run_help.contains("--peer-address"), "node run should accept --peer-address");
    Ok(())
}

#[test]
fn color_option_accepts_all_variants() -> anyhow::Result<()> {
    let amaru = cargo_bin("amaru");
    for variant in &["auto", "always", "never", "on", "off"] {
        let mut command = Command::new(&amaru);
        command.arg("--color").arg(variant).arg("--help");
        let output = command.output()?;
        assert!(output.status.success(), "amaru --color {variant} --help should succeed");
    }
    Ok(())
}

#[test]
fn no_short_options_on_dump_chain_db() -> anyhow::Result<()> {
    let help = amaru_help(&["dev", "chain", "dump"])?;
    assert!(!help.contains("  -H"), "dump should not have -H short option");
    assert!(!help.contains("  -B"), "dump should not have -B short option");
    assert!(help.contains("--headers"), "dump should have --headers long option");
    assert!(help.contains("--blocks"), "dump should have --blocks long option");
    Ok(())
}
