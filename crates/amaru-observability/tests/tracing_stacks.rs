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

//! Shape contracts for each complete product tracing stack.
//!
//! These tests emit one shared nested-span fixture and assert the encoding
//! documented in EDR-033. They compose the same layers the product binary uses:
//!
//! - console: [`console_field_formatter`] + [`CborConsoleEventFormat`]
//! - JSON: [`CborJsonSpanLayer`] + [`CborJsonEventFormat`]
//! - OTEL: `tracing-opentelemetry` + [`CborTraceArrayLayer`] + [`CborOtelLogBridge`]
//! - TUI: [`TelemetryCaptureLayer`]

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use amaru_observability::{
    CborConsoleEventFormat, CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, CborOtelLogBridge,
    CborTraceArrayLayer, FieldValue, TelemetryCaptureLayer, TelemetryRecord, console_field_formatter, encode_cbor,
};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    logs::{InMemoryLogExporter, SdkLoggerProvider},
    trace::{InMemorySpanExporter, SdkTracerProvider},
};
use tracing::Subscriber;
use tracing_subscriber::{
    Layer,
    fmt::{MakeWriter, format::FmtSpan},
    layer::SubscriberExt,
};

const HASH: &str = "3bc8f4b70575ead11872b035b6b95561b43bc7db5b5c0e304ba65e1f65eab5f2";
const OUTER: &str = "epoch.transition";
const MID: &str = "governance.ratify_proposals";
const WRAP: &str = "ratification.round";
const EVENT: &str = "ratification.summarize";

/// Three nested spans + an event inside the innermost span.
///
/// Mirrors a typical ledger epoch-boundary sequence. Three levels are required
/// so the abbreviated ancestor path is `e.t:g.r` once the wrapping span is
/// excluded from `parents`.
fn emit_nested_fixture() {
    let hash = encode_cbor(&HASH);
    let peers = encode_cbor(&vec!["a:1".to_string(), "b:2".to_string()]);
    let outer = tracing::info_span!(OUTER, from = 599_u64, into = 600_u64, "amaru.tag.cpu" = true,);
    let _outer = outer.enter();
    let mid = tracing::info_span!(MID, epoch = 598_u64, "amaru.tag.cpu" = true,);
    let _mid = mid.enter();
    let wrap = tracing::info_span!(
        WRAP,
        header_hash = hash.as_ref() as &[u8],
        votes = tracing::field::Empty,
        "amaru.tag.cpu" = true,
    );
    let _wrap = wrap.enter();
    wrap.record("votes", 372_u64);
    tracing::info!(
        target: "amaru::ledger",
        is_dormant_epoch = false,
        peers = peers.as_ref() as &[u8],
        "{EVENT}"
    );
}

#[derive(Clone, Default)]
struct CaptureWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    fn lock_buf(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.buffer.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.lock_buf()).into_owned()
    }
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock_buf().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn console_subscriber(writer: CaptureWriter) -> impl Subscriber + Send + Sync {
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .fmt_fields(console_field_formatter())
            .with_span_events(FmtSpan::CLOSE)
            .event_format(CborConsoleEventFormat::new().with_ansi(false))
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
    )
}

fn json_subscriber(writer: CaptureWriter) -> impl Subscriber + Send + Sync {
    tracing_subscriber::registry().with(CborJsonSpanLayer::new()).with(
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_span_events(FmtSpan::ENTER | FmtSpan::EXIT)
            .event_format(CborJsonEventFormat::new())
            .fmt_fields(CborJsonFields::new())
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
    )
}

#[test]
fn console_stack_encodes_nested_spans_and_plain_hash() {
    let writer = CaptureWriter::default();
    let subscriber = console_subscriber(writer.clone());

    tracing::subscriber::with_default(subscriber, emit_nested_fixture);

    let output = writer.contents();
    let event_line = output.lines().find(|l| l.contains(EVENT)).expect("event line");

    assert!(event_line.contains("e.t:g.r"), "abbreviated ancestor path, got: {event_line}");
    assert!(event_line.contains(WRAP), "wrapping span name in full, got: {event_line}");
    assert!(
        !event_line.contains("governance.ratify_proposals"),
        "ancestor names must not be printed in full: {event_line}"
    );
    assert!(
        event_line.contains(&format!("header_hash=\"{HASH}\"")),
        "hash must be a single-quoted hex string, got: {event_line}"
    );
    assert!(
        !event_line.contains(&format!(r#"header_hash="\"{HASH}\"""#)),
        "hash must not be diagnostic-quoted then Debug-quoted: {event_line}"
    );
    assert!(!event_line.contains("amaru.tag"), "console hides schema tags: {event_line}");
    assert!(!event_line.contains("from=599"), "ancestor fields stay on the parent span's own lines: {event_line}");
    assert!(!event_line.contains("epoch=598"), "mid-level ancestor fields are not inlined: {event_line}");
    assert!(event_line.contains("votes=372"), "wrapping span fields are inlined: {event_line}");
    assert!(event_line.contains("is_dormant_epoch=false"), "event fields follow the span path: {event_line}");
    assert!(event_line.contains("a:1"), "CBOR array renders as diagnostic text: {event_line}");
    assert!(event_line.contains("parent_id="), "child lines refer to the parent by id: {event_line}");

    let outer_close = output.lines().find(|l| l.contains("close") && l.contains("from=599")).expect("outer close");
    assert!(!outer_close.contains("e.t"), "root span has no abbreviated ancestors: {outer_close}");
    assert!(outer_close.contains(OUTER), "root span name in full: {outer_close}");
    assert!(outer_close.contains("id="), "span's own close line carries id: {outer_close}");

    // NOTE: whole-entry equality is not as robust as the property checks above;
    // it documents the intended format and will need updating when that format changes.
    assert_eq!(
        redact_console_line(event_line),
        concat!(
            r#"<ts>  INFO amaru::ledger: e.t:g.r:ratification.round ratification.summarize "#,
            r#"is_dormant_epoch=false peers="[\"a:1\", \"b:2\"]" "#,
            r#"header_hash="3bc8f4b70575ead11872b035b6b95561b43bc7db5b5c0e304ba65e1f65eab5f2" "#,
            r#"votes=372 parent_id=<parent_id>"#,
        ),
    );
}

#[test]
fn json_stack_encodes_span_object_and_ancestry() {
    let writer = CaptureWriter::default();
    let subscriber = json_subscriber(writer.clone());

    tracing::subscriber::with_default(subscriber, emit_nested_fixture);

    let output = writer.contents();
    let event_line = output.lines().find(|l| l.contains(EVENT)).expect("event line");
    let json: serde_json::Value = serde_json::from_str(event_line).expect("json");

    assert_eq!(json["target"], "amaru::ledger");
    assert_eq!(json["fields"]["message"], EVENT);
    assert_eq!(json["fields"]["is_dormant_epoch"], false);
    assert_eq!(json["fields"]["peers"], serde_json::json!(["a:1", "b:2"]));

    assert_eq!(json["span"]["name"], WRAP);
    assert!(json["span"].get("target").and_then(|v| v.as_str()).is_some());
    assert!(json["span"].get("votes").is_none(), "span is identity only");
    assert_eq!(json["fields"]["header_hash"], HASH);
    assert_eq!(json["fields"]["votes"], 372);
    assert_eq!(json["fields"]["amaru.tag.cpu"], true);
    assert!(json["fields"].get("from").is_none(), "ancestor fields are not inlined");
    assert!(json["fields"].get("epoch").is_none(), "mid-level ancestor fields are not inlined");

    assert_eq!(json["parents"], serde_json::json!([OUTER, MID]));
    assert!(json.get("parent_id").and_then(|v| v.as_u64()).is_some());
    assert!(json.get("id").is_none(), "point events do not carry their own span id");

    let enter_wrap = output.lines().find(|l| l.contains("\"enter\"") && l.contains(WRAP)).expect("wrap enter");
    let enter_json: serde_json::Value = serde_json::from_str(enter_wrap).expect("json");
    assert!(enter_json.get("id").and_then(|v| v.as_u64()).is_some());
    assert_eq!(enter_json.get("parent_id"), json.get("parent_id"));
    assert_eq!(enter_json["span"]["name"], WRAP);
    assert_eq!(enter_json["parents"], serde_json::json!([OUTER, MID]));
    assert_eq!(enter_json["fields"]["header_hash"], HASH);

    // NOTE: whole-entry equality is not as robust as the property checks above;
    // it documents the intended format and will need updating when that format changes.
    assert_eq!(
        redact_json_line(event_line),
        concat!(
            r#"{"timestamp":"<ts>","level":"INFO","fields":{"amaru.tag.cpu":true,"#,
            r#""header_hash":"3bc8f4b70575ead11872b035b6b95561b43bc7db5b5c0e304ba65e1f65eab5f2","#,
            r#""is_dormant_epoch":false,"message":"ratification.summarize","peers":["a:1","b:2"],"votes":372},"#,
            r#""target":"amaru::ledger","span":{"name":"ratification.round","target":"tracing_stacks"},"#,
            r#""parents":["epoch.transition","governance.ratify_proposals"],"parent_id":<parent_id>}"#,
        ),
    );
}

#[test]
fn otel_stack_preserves_parent_child_and_plain_hash() {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let tracer_provider = SdkTracerProvider::builder().with_simple_exporter(span_exporter.clone()).build();
    let logger_provider = SdkLoggerProvider::builder().with_simple_exporter(log_exporter.clone()).build();
    let tracer = tracer_provider.tracer("amaru-test");

    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer).with_level(true).with_target(true))
        .with(CborTraceArrayLayer::new())
        .with(CborOtelLogBridge::new(&logger_provider));

    tracing::subscriber::with_default(subscriber, emit_nested_fixture);

    tracer_provider.force_flush().expect("flush traces");
    logger_provider.force_flush().expect("flush logs");

    let spans = span_exporter.get_finished_spans().expect("spans");
    let outer = spans.iter().find(|s| s.name == OUTER).expect("outer span");
    let mid = spans.iter().find(|s| s.name == MID).expect("mid span");
    let wrap = spans.iter().find(|s| s.name == WRAP).expect("wrap span");
    assert_eq!(mid.parent_span_id, outer.span_context.span_id());
    assert_eq!(wrap.parent_span_id, mid.span_context.span_id());
    assert_eq!(wrap.span_context.trace_id(), outer.span_context.trace_id());

    // Stock tracing-opentelemetry may also store a hex dump of the CBOR bytes.
    // CborTraceArrayLayer appends the upgraded value; exporters keep the last key.
    let header = wrap
        .attributes
        .iter()
        .rev()
        .find(|kv| kv.key.as_str() == "header_hash")
        .map(|kv| kv.value.as_str().to_string())
        .expect("header_hash attribute");
    assert_eq!(header, HASH, "OTEL hash must be the plain hex string, not diagnostic quotes");
    assert!(!header.contains('\\') && !header.contains('"'), "hash must not be quoted: {header}");

    let epoch = mid.attributes.iter().find(|kv| kv.key.as_str() == "epoch").map(|kv| kv.value.clone());
    assert!(epoch.is_some(), "typed span fields become OTEL attributes");

    let logs = log_exporter.get_emitted_logs().expect("logs");
    let event = logs.iter().find(|l| l.record.body().is_some_and(any_value_is_string(EVENT))).expect("event log");
    let trace_context = event.record.trace_context().expect("log joined to current span");
    assert_eq!(trace_context.trace_id, wrap.span_context.trace_id());
    assert_eq!(trace_context.span_id, wrap.span_context.span_id());

    let peers = event
        .record
        .attributes_iter()
        .find(|(k, _)| k.as_str() == "peers")
        .map(|(_, v)| v.clone())
        .expect("peers attr");
    let opentelemetry::logs::AnyValue::ListAny(list) = peers else {
        panic!("peers must be a nested list, got {peers:?}");
    };
    assert_eq!(list.len(), 2);

    // NOTE: whole-entry equality is not as robust as the property checks above;
    // it documents the intended format and will need updating when that format changes.
    assert_eq!(
        format_otel_example(wrap, &event.record),
        concat!(
            "span name=ratification.round\n",
            "  header_hash=3bc8f4b70575ead11872b035b6b95561b43bc7db5b5c0e304ba65e1f65eab5f2\n",
            "  votes=372\n",
            "  amaru.tag.cpu=true\n",
            "log body=ratification.summarize target=amaru::ledger\n",
            "  is_dormant_epoch=false\n",
            "  peers=[a:1, b:2]\n",
            "  trace_id=<trace_id> span_id=<span_id>",
        ),
    );
}

fn any_value_is_string(expected: &'static str) -> impl Fn(&opentelemetry::logs::AnyValue) -> bool {
    move |value| match value {
        opentelemetry::logs::AnyValue::String(s) => s.as_str() == expected,
        opentelemetry::logs::AnyValue::Int(_)
        | opentelemetry::logs::AnyValue::Double(_)
        | opentelemetry::logs::AnyValue::Boolean(_)
        | opentelemetry::logs::AnyValue::Bytes(_)
        | opentelemetry::logs::AnyValue::ListAny(_)
        | opentelemetry::logs::AnyValue::Map(_) => false,
        // AnyValue is non-exhaustive
        _ => false,
    }
}

#[test]
fn tui_stack_records_parents_and_inlined_fields() {
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

    tracing::subscriber::with_default(subscriber, emit_nested_fixture);

    let records: Vec<_> = rx.try_iter().collect();
    let event = records
        .iter()
        .find(|r| r.duration.is_none() && r.fields.contains_key("is_dormant_epoch"))
        .unwrap_or_else(|| panic!("event, got {records:#?}"));
    assert_eq!(event.parents, vec![OUTER.to_string(), MID.to_string()]);
    assert_eq!(event.span_name.as_deref(), Some(WRAP));
    assert_eq!(event.fields.get("is_dormant_epoch"), Some(&FieldValue::Bool(false)));
    assert_eq!(event.fields.get("votes"), Some(&FieldValue::U64(372)), "wrapping span fields are inlined");
    assert!(!event.fields.contains_key("from"), "ancestor fields are not inlined");
    assert!(!event.fields.contains_key("epoch"), "mid-level ancestor fields are not inlined");
    assert!(event.parent_id.is_some());
    assert!(event.id.is_none());
    match event.fields.get("peers") {
        Some(FieldValue::Str(s)) => assert!(s.contains("a:1"), "peers diagnostic={s}"),
        other => panic!("peers should be diagnostic text, got {other:?}"),
    }
    assert!(!event.fields.keys().any(|k| k.starts_with("amaru.tag.")));

    let closed_wrap = records.iter().find(|r| r.name == WRAP && r.duration.is_some()).expect("wrap close");
    assert_eq!(closed_wrap.parents, vec![OUTER.to_string(), MID.to_string()]);
    assert_eq!(closed_wrap.span_name.as_deref(), Some(WRAP));
    assert_eq!(closed_wrap.fields.get("header_hash"), Some(&FieldValue::Str(HASH.into())));
    assert_eq!(closed_wrap.fields.get("votes"), Some(&FieldValue::U64(372)));
    assert_eq!(closed_wrap.parent_id, event.parent_id);
    assert!(closed_wrap.id.is_some());
    assert!(!closed_wrap.fields.keys().any(|k| k.starts_with("amaru.tag.")));

    // NOTE: whole-entry equality is not as robust as the property checks above;
    // it documents the intended format and will need updating when that format changes.
    assert_eq!(
        format_tui_example(event),
        concat!(
            "parents=[epoch.transition, governance.ratify_proposals] span_name=ratification.round\n",
            "  message=ratification.summarize is_dormant_epoch=false ",
            r#"peers=["a:1", "b:2"] "#,
            "header_hash=3bc8f4b70575ead11872b035b6b95561b43bc7db5b5c0e304ba65e1f65eab5f2 votes=372\n",
            "  parent_id=<parent_id>",
        ),
    );
}

fn redact_leading_ts(line: &str) -> String {
    match line.find('Z') {
        Some(end) => format!("<ts>{}", &line[end + 1..]),
        None => line.to_string(),
    }
}

fn redact_numeric_field(s: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let Some(pos) = s.find(&needle) else {
        return s.to_string();
    };
    let start = pos + needle.len();
    let digits = s[start..].bytes().take_while(u8::is_ascii_digit).count();
    format!("{}<{name}>{}", &s[..start], &s[start + digits..])
}

fn redact_console_line(line: &str) -> String {
    redact_numeric_field(&redact_leading_ts(line), "parent_id")
}

fn redact_json_line(line: &str) -> String {
    let timestamp = "\"timestamp\":\"";
    let Some(ts_at) = line.find(timestamp) else {
        return line.to_string();
    };
    let ts_value = ts_at + timestamp.len();
    let Some(ts_end) = line[ts_value..].find('"') else {
        return line.to_string();
    };
    let with_ts = format!("{}<ts>{}", &line[..ts_value], &line[ts_value + ts_end..]);

    let parent = "\"parent_id\":";
    let Some(id_at) = with_ts.find(parent) else {
        return with_ts;
    };
    let id_value = id_at + parent.len();
    let digits = with_ts[id_value..].bytes().take_while(u8::is_ascii_digit).count();
    format!("{}<parent_id>{}", &with_ts[..id_value], &with_ts[id_value + digits..])
}

fn last_span_attr(span: &opentelemetry_sdk::trace::SpanData, key: &str) -> String {
    span.attributes
        .iter()
        .rev()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| match &kv.value {
            opentelemetry::Value::Bool(value) => value.to_string(),
            opentelemetry::Value::I64(value) => value.to_string(),
            opentelemetry::Value::F64(value) => value.to_string(),
            opentelemetry::Value::String(value) => value.as_str().to_string(),
            other @ opentelemetry::Value::Array(_) | other => format!("{other:?}"),
        })
        .unwrap_or_default()
}

fn any_value_example(value: &opentelemetry::logs::AnyValue) -> String {
    match value {
        opentelemetry::logs::AnyValue::Boolean(value) => value.to_string(),
        opentelemetry::logs::AnyValue::Int(value) => value.to_string(),
        opentelemetry::logs::AnyValue::Double(value) => value.to_string(),
        opentelemetry::logs::AnyValue::String(value) => value.as_str().to_string(),
        opentelemetry::logs::AnyValue::ListAny(list) => {
            let items = list.iter().map(any_value_example).collect::<Vec<_>>().join(", ");
            format!("[{items}]")
        }
        other @ opentelemetry::logs::AnyValue::Bytes(_) | other @ opentelemetry::logs::AnyValue::Map(_) | other => {
            format!("{other:?}")
        }
    }
}

fn log_attr<'a>(
    record: &'a opentelemetry_sdk::logs::SdkLogRecord,
    key: &str,
) -> Option<&'a opentelemetry::logs::AnyValue> {
    record.attributes_iter().find(|(k, _)| k.as_str() == key).map(|(_, value)| value)
}

fn format_otel_example(
    wrap: &opentelemetry_sdk::trace::SpanData,
    record: &opentelemetry_sdk::logs::SdkLogRecord,
) -> String {
    let body = record.body().map(any_value_example).unwrap_or_default();
    let target = record.target().map(|t| t.as_ref()).unwrap_or_default();
    let dormant = log_attr(record, "is_dormant_epoch").map(any_value_example).unwrap_or_default();
    let peers = log_attr(record, "peers").map(any_value_example).unwrap_or_default();
    format!(
        "span name={}\n  header_hash={}\n  votes={}\n  amaru.tag.cpu={}\nlog body={body} target={target}\n  is_dormant_epoch={dormant}\n  peers={peers}\n  trace_id=<trace_id> span_id=<span_id>",
        wrap.name,
        last_span_attr(wrap, "header_hash"),
        last_span_attr(wrap, "votes"),
        last_span_attr(wrap, "amaru.tag.cpu"),
    )
}

fn format_tui_example(event: &TelemetryRecord) -> String {
    let message = event.fields.get("message").map(ToString::to_string).unwrap_or_default();
    let dormant = event.fields.get("is_dormant_epoch").map(ToString::to_string).unwrap_or_default();
    let peers = event.fields.get("peers").map(ToString::to_string).unwrap_or_default();
    let hash = event.fields.get("header_hash").map(ToString::to_string).unwrap_or_default();
    let votes = event.fields.get("votes").map(ToString::to_string).unwrap_or_default();
    let span_name = event.span_name.as_deref().unwrap_or_default();
    format!(
        "parents=[{}] span_name={span_name}\n  message={message} is_dormant_epoch={dormant} peers={peers} header_hash={hash} votes={votes}\n  parent_id=<parent_id>",
        event.parents.join(", "),
    )
}
