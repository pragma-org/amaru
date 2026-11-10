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

use std::fs::{File, read_to_string};

use amaru_node::telemetry::{LocalTelemetry, OtelSignals};
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
