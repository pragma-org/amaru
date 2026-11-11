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

//! Output contracts for the CLI telemetry configuration, including simultaneous file sinks.
//!
//! Expected output was captured using the CLI's `TracingSubscriber` at
//! bd286c86357333144b0c565ad68f95a539c40d37 with the same fixture and filters.

#[path = "telemetry_fixture.rs"]
mod telemetry_fixture;

use std::{
    env,
    fs::{File, read_to_string},
    thread,
    time::Duration,
};

use amaru_node::telemetry::{LocalTelemetry, OtelSignal, OtelSignals};
use amaru_observability::{
    subscribe_telemetry, tracing,
    tracing_subscriber::{EnvFilter, fmt::writer::BoxMakeWriter},
};
use serde_json::{Value, json};
use test_case::test_case;

use self::telemetry_fixture::{FILTER, normalize_json, normalize_text, normalize_tui, run_ledger_fixture};
use super::{ObservabilityHints, observability_config};

struct NoListenAddress;

impl ObservabilityHints for NoListenAddress {
    fn listen_address(&self) -> Option<&str> {
        None
    }
}

#[test_case("console", include_str!("fixtures/telemetry/console.json"); "console")]
#[test_case("json", include_str!("fixtures/telemetry/json.json"); "json")]
#[test_case("tui", include_str!("fixtures/telemetry/tui.json"); "tui")]
fn telemetry_output_matches_contract(mode: &str, expected: &str) {
    let directory = tempfile::tempdir().unwrap();
    let text_path = directory.path().join("trace.log");
    let json_path = directory.path().join("trace.ndjson");
    let output_path = directory.path().join("output");
    let (capture, receiver) = subscribe_telemetry(32);
    let local = LocalTelemetry { capture_layer: (mode == "tui").then_some(capture), ..LocalTelemetry::default() };
    let files = [&text_path, &json_path].map(|path| format!("{}:{FILTER}", path.display()).parse().unwrap());
    let (mut config, _notices) = observability_config(
        false,
        OtelSignals::default(),
        mode == "json",
        Some(local),
        false,
        &NoListenAddress,
        &files,
    );
    assert_eq!(config.output.is_none(), mode == "tui");
    if let Some(output) = &mut config.output {
        output.writer = BoxMakeWriter::new(File::create(&output_path).unwrap());
        output.filter = EnvFilter::new(FILTER);
    }
    config.capture_filter = EnvFilter::new(FILTER);
    let (subscriber, handle) = config.build().unwrap();
    tracing::subscriber::with_default(subscriber, run_ledger_fixture);
    handle.teardown.shutdown().unwrap();

    let output = match mode {
        "console" => normalize_text(&read_to_string(output_path).unwrap()),
        "json" => normalize_json(&read_to_string(output_path).unwrap()),
        "tui" => normalize_tui(receiver.try_iter().collect()),
        _ => unreachable!(),
    };
    let actual = json!({
        "output": output,
        "text_file": normalize_text(&read_to_string(text_path).unwrap()),
    });
    let expected: Value = serde_json::from_str(expected).unwrap();
    assert_eq!(actual, expected, "{mode} telemetry output changed");
    let expected_file: Value = serde_json::from_str(include_str!("fixtures/telemetry/file.json")).unwrap();
    assert_eq!(normalize_json(&read_to_string(json_path).unwrap()), expected_file, "{mode} JSON file changed");
}

#[test_case(None, None, "info", "debug"; "defaults")]
#[test_case(Some("off"), Some(FILTER), "off", "debug"; "trace_only")]
#[test_case(Some(FILTER), Some("off"), "debug", "off"; "log_only")]
#[test_case(Some("debug"), Some("info"), "debug", "info"; "different_levels")]
#[test_case(Some("off"), Some("off"), "off", "off"; "disabled")]
#[test_case(Some("off,amaru::consensus=debug"), Some("off,amaru::consensus=debug"), "off", "off"; "other_target")]
#[test_case(Some("amaru=invalid"), Some("amaru=invalid"), "info", "debug"; "invalid_falls_back")]
fn environment_filters(log: Option<&str>, trace: Option<&str>, log_level: &str, trace_level: &str) {
    if env::var_os("AMARU_TELEMETRY_TEST_CHILD").is_none() {
        let mut command = assert_cmd::Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", thread::current().name().unwrap(), "--nocapture"])
            .env("AMARU_TELEMETRY_TEST_CHILD", "1")
            .env_remove("AMARU_LOG")
            .env_remove("AMARU_TRACE");
        if let Some(log) = log {
            command.env("AMARU_LOG", log);
        }
        if let Some(trace) = trace {
            command.env("AMARU_TRACE", trace);
        }
        let result = command.timeout(Duration::from_secs(30)).assert().success();
        assert!(String::from_utf8_lossy(&result.get_output().stdout).contains("1 passed"));
        return;
    }

    assert_otlp_signal_selection();
    for mode in ["console", "ansi", "json", "tui", "tui-json"] {
        for file_levels in [["off", "debug"], ["info", "off"], ["debug", "info"], ["off", "off"]] {
            let directory = tempfile::tempdir().unwrap();
            let output_path = directory.path().join("output");
            let paths = [directory.path().join("trace.log"), directory.path().join("trace.ndjson")];
            let files: Vec<_> = paths
                .iter()
                .zip(file_levels)
                .map(|(path, level)| format!("{}:{level}", path.display()).parse().unwrap())
                .collect();
            let (capture, receiver) = subscribe_telemetry(32);
            let tui = mode.starts_with("tui");
            let json = mode.ends_with("json");
            let local = LocalTelemetry { capture_layer: tui.then_some(capture), ..LocalTelemetry::default() };
            let (mut config, _) = observability_config(
                false,
                OtelSignals::default(),
                json,
                Some(local),
                mode == "ansi",
                &NoListenAddress,
                &files,
            );
            assert_eq!(config.output.is_some(), !tui || json);
            if let Some(output) = &mut config.output {
                output.writer = BoxMakeWriter::new(File::create(&output_path).unwrap());
            }
            let (subscriber, handle) = config.build().unwrap();
            tracing::subscriber::with_default(subscriber, run_ledger_fixture);
            handle.teardown.shutdown().unwrap();

            if !tui || json {
                let output = read_to_string(&output_path).unwrap();
                assert_ledger_filter(&output, if json { trace_level } else { log_level });
                assert_eq!(output.contains('\u{1b}'), mode == "ansi" && log_level != "off");
            }
            let captured = receiver.try_iter().map(|record| record.name).collect::<Vec<_>>().join("\n");
            assert_ledger_filter(&captured, if tui { trace_level } else { "off" });
            for (path, level) in paths.into_iter().zip(file_levels) {
                assert_ledger_filter(&read_to_string(path).unwrap(), level);
            }
        }
    }
}

fn assert_ledger_filter(output: &str, level: &str) {
    assert_eq!(output.is_empty(), level == "off", "expected {level}: {output}");
    assert_eq!(output.contains("proposal.drop"), level != "off", "expected {level}: {output}");
    assert_eq!(output.contains("proposal_roots.summarize"), level == "debug", "expected {level}: {output}");
    assert_eq!(output.contains("governance.ratifying"), level == "debug", "expected {level}: {output}");
}

fn assert_otlp_signal_selection() {
    use OtelSignal::{Logs, Metrics, Traces};

    for enabled in [false, true] {
        for (signals, uses_trace_filter) in [
            (vec![], false),
            (vec![Metrics], false),
            (vec![Traces], true),
            (vec![Logs], true),
            (vec![Metrics, Traces], true),
            (vec![Metrics, Logs], true),
            (vec![Traces, Logs], true),
            (vec![Metrics, Traces, Logs], true),
        ] {
            let signals = OtelSignals(signals);
            let (config, _) = observability_config(enabled, signals.clone(), false, None, false, &NoListenAddress, &[]);
            assert_eq!(config.otlp.is_some(), enabled);
            if let Some(otlp) = config.otlp {
                assert_eq!(otlp.signals, signals);
                let expected = if uses_trace_filter {
                    amaru_node::telemetry::filter_from_env("AMARU_TRACE", super::DEFAULT_AMARU_TRACE_FILTER).0
                } else {
                    EnvFilter::new("off")
                };
                assert_eq!(otlp.filter.to_string(), expected.to_string());
            }
        }
    }
}
