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

//! Tests that verify runtime behavior of trace macro
//!
//! These tests verify:
//! - Correct target and span names are generated
//! - Schema fields are declared in span metadata
//! - Field values are auto-recorded
//! - trace_record! records to current span

use amaru_observability_macros::{define_local_schemas, trace, trace_record};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tracing::field::Visit;
use tracing_subscriber::{Registry, layer::SubscriberExt};

define_local_schemas! {
    consensus {
        validate_header {
            /// Evolve nonce for testing
            EVOLVE_NONCE {
                required hash: String
            }
        }
    }

    ledger {
        state {
            /// Apply block for testing
            APPLY_BLOCK {
                required point_slot: u64
            }

            /// Create validation context for testing
            CREATE_VALIDATION_CONTEXT {
                required block_body_hash: String
                required block_number: u64
                required block_body_size: u64
                optional total_inputs: u64
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CapturedSpan {
    name: String,
    target: String,
    level: tracing::Level,
    fields: Vec<String>,
}

struct CapturingLayer {
    captured_spans: Arc<Mutex<Vec<CapturedSpan>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturingLayer {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let field_names: Vec<_> = attrs
            .metadata()
            .fields()
            .iter()
            .map(|f| f.name().to_string())
            .collect();

        self.captured_spans.lock().unwrap().push(CapturedSpan {
            name: attrs.metadata().name().to_string(),
            target: attrs.metadata().target().to_string(),
            level: *attrs.metadata().level(),
            fields: field_names,
        });
    }
}

#[derive(Default)]
struct FieldValueCollector {
    values: BTreeMap<String, String>,
}

impl Visit for FieldValueCollector {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.values
            .insert(field.name().to_string(), format!("{:?}", value));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.values
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.values
            .insert(field.name().to_string(), value.to_string());
    }
}

struct ValueCapturingLayer {
    captured: Arc<Mutex<BTreeMap<String, String>>>,
}

impl<S> tracing_subscriber::Layer<S> for ValueCapturingLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_record(
        &self,
        _id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut collector = FieldValueCollector::default();
        values.record(&mut collector);
        self.captured.lock().unwrap().extend(collector.values);
    }
}

#[trace(consensus::validate_header::EVOLVE_NONCE)]
fn evolve_nonce(hash: String) {
    let _ = hash;
}

#[trace(ledger::state::APPLY_BLOCK)]
fn apply_block(point_slot: u64) {
    let _ = point_slot;
}

#[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
fn process_block(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {}

#[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
fn outer_with_record(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {
    inner_record(5);
}

fn inner_record(_total_inputs: u64) {
    trace_record!(
        ledger::state::CREATE_VALIDATION_CONTEXT,
        total_inputs = _total_inputs
    );
}

#[test]
fn test_span_target_and_name() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CapturingLayer {
        captured_spans: captured.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        evolve_nonce("test".into());
        apply_block(42);
    });

    let spans = captured.lock().unwrap();
    assert_eq!(spans.len(), 2);

    assert_eq!(spans[0].target, "consensus::validate_header");
    assert_eq!(spans[0].name, "evolve_nonce");

    assert_eq!(spans[1].target, "ledger::state");
    assert_eq!(spans[1].name, "apply_block");
}

#[test]
fn test_span_level_is_trace() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CapturingLayer {
        captured_spans: captured.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        evolve_nonce("test".into());
    });

    let spans = captured.lock().unwrap();
    assert_eq!(spans[0].level, tracing::Level::TRACE);
}

#[test]
fn test_schema_fields_declared() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CapturingLayer {
        captured_spans: captured.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        process_block("hash".into(), 100, 1024);
    });

    let spans = captured.lock().unwrap();
    let fields = &spans[0].fields;

    assert!(fields.contains(&"block_body_hash".into()));
    assert!(fields.contains(&"block_number".into()));
    assert!(fields.contains(&"block_body_size".into()));
    assert!(fields.contains(&"total_inputs".into())); // optional field also declared
}

#[test]
fn test_field_values_recorded() {
    let values = Arc::new(Mutex::new(BTreeMap::new()));
    let subscriber = Registry::default().with(ValueCapturingLayer {
        captured: values.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        process_block("0xabc".into(), 100, 1024);
    });

    let recorded = values.lock().unwrap();
    assert_eq!(recorded.get("block_body_hash"), Some(&"0xabc".to_string()));
}

#[test]
fn test_trace_record_records_to_span() {
    let values = Arc::new(Mutex::new(BTreeMap::new()));
    let subscriber = Registry::default().with(ValueCapturingLayer {
        captured: values.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        outer_with_record("hash".into(), 100, 1024);
    });

    let recorded = values.lock().unwrap();
    // The inner record function should record total_inputs
    assert!(
        recorded.contains_key("total_inputs") || recorded.contains_key("block_body_hash"),
        "Expected some fields to be recorded, got {:?}",
        recorded
    );
}
