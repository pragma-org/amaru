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

//! Header spans exported from one follower syncing a generated chain.

use std::{
    num::NonZeroU8,
    time::{Duration, SystemTime},
};

use amaru_consensus::performance::close_root_forward_after_exit;
use amaru_kernel::{Header, HeaderHash, IsHeader};
use amaru_observability::{CborOtelLogBridge, CborTraceArrayLayer, SpanDurationLayer};
use opentelemetry::{logs::AnyValue, trace::TracerProvider as _};
use opentelemetry_sdk::{
    logs::{InMemoryLogExporter, SdkLoggerProvider, in_memory_exporter::LogDataWithResource},
    trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor, SpanData},
};
use tracing_subscriber::{
    Layer,
    layer::{Filter, SubscriberExt},
};

use super::generated::{BLOCKFETCH_HORIZON_NANOS, SyncRun, spawn_blockfetch_follower};
use crate::telemetry::{CborSpanExporter, NotingSpanProcessor};

const BLOCKPERF: [&str; 4] = ["header.announced", "block.requested", "block.received", "block.adopted"];

struct SpansOnly;

impl<S> Filter<S> for SpansOnly {
    fn enabled(&self, meta: &tracing::Metadata<'_>, _: &tracing_subscriber::layer::Context<'_, S>) -> bool {
        meta.is_span()
    }
}

/// One follower, the generated-chain injector, and the product trace layers on that follower only.
///
/// Not `#[tokio::test]`: `build_node` zero-duration effects `block_on` the runtime handle.
/// Private schemas stay off: `roll_forward.process` is the exported parent of the forward span.
#[test]
fn test_world_header_span_export() {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_span_processor(NotingSpanProcessor::new(SimpleSpanProcessor::new(CborSpanExporter::new(
            span_exporter.clone(),
        ))))
        .build();
    let logger_provider = SdkLoggerProvider::builder().with_simple_exporter(log_exporter.clone()).build();
    let tracer = tracer_provider.tracer("amaru");
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_level(true)
                .with_target(true)
                .with_filter(SpansOnly),
        )
        .with(SpanDurationLayer::new())
        .with(CborTraceArrayLayer::new())
        .with(CborOtelLogBridge::new(&logger_provider).with_filter(tracing_subscriber::filter::LevelFilter::DEBUG));

    let run = SyncRun::new("header span export");
    let (mut world, headers) = spawn_blockfetch_follower(&run, 9740, NonZeroU8::MIN);
    world = world.with_graph_tracing(1, tracing::Dispatch::new(subscriber));
    world.run_until_horizon_on_best_chain_tip(BLOCKFETCH_HORIZON_NANOS, |_| {});

    tracer_provider.force_flush().expect("flush spans");
    logger_provider.force_flush().expect("flush logs");
    let spans = span_exporter.get_finished_spans().expect("spans");
    let logs = log_exporter.get_emitted_logs().expect("logs");

    let (hash, forward, fetch_wait, fetch) = headers
        .iter()
        .skip(1)
        .map(Header::hash)
        .map(|hash| hash.to_string())
        .find_map(|hash| {
            let forward = span_named(&spans, "perf.header.forward", &hash)?;
            let fetch_wait = span_named(&spans, "perf.header.block_fetch_wait", &hash)?;
            let fetch = span_named(&spans, "perf.blocks.fetch", &hash)?;
            Some((hash, forward, fetch_wait, fetch))
        })
        .unwrap_or_else(|| panic!("no adopted header exported forward, fetch wait, and fetch; spans={spans:#?}"));
    let roll_forward = span_named(&spans, "roll_forward.process", &hash).expect("roll_forward.process");

    assert_eq!(forward.span_context.trace_id(), roll_forward.span_context.trace_id());
    assert_eq!(forward.parent_span_id, roll_forward.span_context.span_id());
    assert_eq!(fetch_wait.parent_span_id, forward.span_context.span_id());
    assert_eq!(fetch.parent_span_id, forward.span_context.span_id());
    assert!(fetch_wait.end_time <= fetch.start_time, "fetch wait must end before fetch starts");
    assert!(fetch.end_time <= forward.end_time, "fetch must end before forward ends");

    let hash_attrs: Vec<_> = forward.attributes.iter().filter(|kv| kv.key.as_str() == "header_hash").collect();
    assert_eq!(hash_attrs.len(), 1, "forward span must carry one header_hash, got {hash_attrs:?}");
    let opentelemetry::Value::String(text) = &hash_attrs[0].value else {
        panic!("header_hash must be a string, got {:?}", hash_attrs[0].value);
    };
    assert_eq!(text.as_str(), hash);
    assert!(text.as_str().chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

    let trace_id = forward.span_context.trace_id();
    for name in BLOCKPERF {
        let log = logs
            .iter()
            .find(|log| log.record.event_name() == Some(name) && log_has_hash(log, &hash))
            .unwrap_or_else(|| panic!("missing blockperf {name} for {hash}"));
        let context = log.record.trace_context().unwrap_or_else(|| panic!("{name} has an empty trace context"));
        assert_eq!(context.trace_id, trace_id, "{name} must join the forward trace");
        assert!(context.span_id != opentelemetry::trace::SpanId::INVALID, "{name} span id must be set");
    }

    let lifecycle = logs
        .iter()
        .find(|log| log.record.event_name() == Some("perf.header.lifecycle") && log_has_hash(log, &hash))
        .expect("lifecycle");
    assert_eq!(log_u64(lifecycle, "forward_micros"), Some(elapsed_micros(forward)));
    assert_eq!(log_u64(lifecycle, "block_fetch_wait_micros"), Some(elapsed_micros(fetch_wait)));
    assert_eq!(log_u64(lifecycle, "block_fetch_micros"), Some(elapsed_micros(fetch)));

    let mut carrying = Vec::new();
    for span in spans.iter().filter(|span| span_has_hash(span, &hash)) {
        carrying.push(format!("span {}", span.name));
    }
    for log in logs.iter().filter(|log| log_has_hash(log, &hash)) {
        carrying.push(format!("log {}", log.record.event_name().unwrap_or("")));
    }
    carrying.sort();
    let callsites: Vec<_> = carrying.iter().filter(|name| name.starts_with("log event ")).cloned().collect();
    let mut stable: Vec<_> = carrying.iter().filter(|name| !name.starts_with("log event ")).cloned().collect();
    assert_eq!(
        callsites.len(),
        1,
        "roll-forward field record should be the only callsite log for {hash}: {callsites:?}"
    );
    assert!(
        callsites[0].contains("track_peers"),
        "callsite log should be the roll-forward field record, got {}",
        callsites[0]
    );
    let mut expected = vec![
        "log block.adopted",
        "log block.received",
        "log block.requested",
        "log header.announced",
        "log perf.header.lifecycle",
        "log tip.adopt",
        "span chain.fetch_next",
        "span chain.select_from_block_validation",
        "span chain.select_from_tip",
        "span perf.blocks.fetch",
        "span perf.header.block_fetch_wait",
        "span perf.header.forward",
        "span roll_forward.process",
    ];
    expected.sort();
    stable.sort();
    assert_eq!(stable, expected, "complete export for {hash}");

    let rest = spans.iter().filter(|span| !span_has_hash(span, &hash)).count()
        + logs.iter().filter(|log| !log_mentions_hash(log, &hash)).count();
    assert!(rest > 0, "export must keep traces that are not this header");
}

fn span_named<'a>(spans: &'a [SpanData], name: &str, hash: &str) -> Option<&'a SpanData> {
    spans.iter().find(|span| span.name == name && span_has_hash(span, hash))
}

fn span_has_hash(span: &SpanData, hash: &str) -> bool {
    span.attributes.iter().any(|kv| kv.key.as_str() == "header_hash" && value_is_hash(&kv.value, hash))
        || span.events.events.iter().any(|event| {
            event.attributes.iter().any(|kv| kv.key.as_str() == "header_hash" && value_is_hash(&kv.value, hash))
        })
}

fn value_is_hash(value: &opentelemetry::Value, hash: &str) -> bool {
    match value {
        opentelemetry::Value::String(text) => text.as_str() == hash,
        opentelemetry::Value::Bool(_)
        | opentelemetry::Value::I64(_)
        | opentelemetry::Value::F64(_)
        | opentelemetry::Value::Array(_)
        | _ => false,
    }
}

fn log_has_hash(log: &LogDataWithResource, hash: &str) -> bool {
    log.record.attributes_iter().any(|(key, value)| key.as_str() == "header_hash" && any_is_hash(value, hash))
}

fn log_mentions_hash(log: &LogDataWithResource, hash: &str) -> bool {
    log_has_hash(log, hash) || log.record.body().is_some_and(|body| any_is_hash(body, hash))
}

fn any_is_hash(value: &AnyValue, hash: &str) -> bool {
    match value {
        AnyValue::String(text) => text.as_str() == hash,
        AnyValue::Bytes(bytes) => hex::encode(bytes.as_ref()) == hash,
        AnyValue::Int(_) | AnyValue::Double(_) | AnyValue::Boolean(_) | AnyValue::ListAny(_) | AnyValue::Map(_) => {
            false
        }
        _ => false,
    }
}

fn log_u64(log: &LogDataWithResource, key: &str) -> Option<u64> {
    log.record.attributes_iter().find(|(name, _)| name.as_str() == key).and_then(|(_, value)| match value {
        AnyValue::Int(value) if *value >= 0 => u64::try_from(*value).ok(),
        AnyValue::String(text) => text.as_str().parse().ok(),
        AnyValue::Int(_)
        | AnyValue::Double(_)
        | AnyValue::Boolean(_)
        | AnyValue::Bytes(_)
        | AnyValue::ListAny(_)
        | AnyValue::Map(_)
        | _ => None,
    })
}

fn elapsed_micros(span: &SpanData) -> u64 {
    span.end_time.duration_since(span.start_time).unwrap_or_default().as_micros() as u64
}

/// Enter and exit the forward span the way a blockperf line does, then close it later.
///
/// The exported end is the close. An exit alone would leave the span ended at that line.
#[test]
fn test_forward_span_ends_at_close_not_blockperf_exit() {
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider =
        SdkTracerProvider::builder().with_simple_exporter(CborSpanExporter::new(span_exporter.clone())).build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("amaru")).with_level(true));

    let hash = HeaderHash::new([0xab; 32]);
    let mut exited_at = SystemTime::UNIX_EPOCH;
    tracing::subscriber::with_default(subscriber, || {
        close_root_forward_after_exit(hash, || {
            exited_at = SystemTime::now();
            std::thread::sleep(Duration::from_millis(40));
        });
    });

    tracer_provider.force_flush().expect("flush spans");
    let spans = span_exporter.get_finished_spans().expect("spans");
    let forward = spans.iter().find(|span| span.name == "perf.header.forward").expect("forward span");
    let since_exit = forward.end_time.duration_since(exited_at).unwrap_or_default();
    assert!(
        since_exit > Duration::from_millis(20),
        "forward span ended at the blockperf exit ({since_exit:?} after it), end={:?} exit={exited_at:?}",
        forward.end_time
    );
}
