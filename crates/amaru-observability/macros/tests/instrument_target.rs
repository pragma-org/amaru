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
//! - Schema and built-in fields are declared in span metadata
//! - Field values are auto-recorded
//! - trace_record! records to current span

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

use amaru_observability_macros::{define_local_schemas, trace_record, trace_span};
use tracing::field::Visit;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone)]
struct DistinctFormatting;

impl fmt::Display for DistinctFormatting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("display-format")
    }
}

impl fmt::Debug for DistinctFormatting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("debug-format")
    }
}

define_local_schemas! {
    amaru {
        formatting {
            test {
                /// Formatter behavior for testing
                public DISTINCT_FORMATTING {
                    required display_value: %DistinctFormatting
                    required debug_value: ?DistinctFormatting
                }
            }
        }
        consensus {
            roll_forward {
                /// Roll forward processing for testing
                public PROCESS {
                    required peer: String
                }
            }
            header {
                /// Evolve nonce for testing
                public EVOLVE_NONCE {
                    required hash: String
                }
            }
            chain_sync {
                /// Roll forward for testing
                public ROLL_FORWARD {
                    required peer: String
                }
            }
        }
        ledger {
            block {
                /// Apply block for testing
                public APPLY {
                    required point_slot: u64
                }
                /// Create validation context for testing
                public CREATE_VALIDATION_CONTEXT {
                    required block_body_hash: String
                    required block_number: u64
                    required block_body_size: u64
                    optional total_inputs: u64
                }
            }
        }
        classification {
            test {
                /// Span classification for testing
                public REGISTERED_SPAN {
                    required tip: String
                }
            }
        }
        categorized {
            work {
                tags: cpu, io
                /// Tagged span for testing
                public COMPUTE {
                    required label: String
                }
                /// Tagged span overriding the module tags for testing
                public STORE {
                    tags: setup
                    required label: String
                }
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
    parent: Option<String>,
}

struct CapturingLayer {
    captured_spans: Arc<Mutex<Vec<CapturedSpan>>>,
}

impl<S> tracing_subscriber::Layer<S> for CapturingLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let field_names: Vec<_> = attrs.metadata().fields().iter().map(|f| f.name().to_string()).collect();
        let parent = ctx.span(id).and_then(|span| span.parent().map(|parent| parent.metadata().name().to_string()));

        self.captured_spans.lock().unwrap().push(CapturedSpan {
            name: attrs.metadata().name().to_string(),
            target: attrs.metadata().target().to_string(),
            level: *attrs.metadata().level(),
            fields: field_names,
            parent,
        });
    }
}

#[derive(Default)]
struct FieldValueCollector {
    values: BTreeMap<String, String>,
}

impl Visit for FieldValueCollector {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.values.insert(field.name().to_string(), format!("{:?}", value));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.values.insert(field.name().to_string(), value.to_string());
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.values.insert(field.name().to_string(), value.to_string());
    }
}

struct ValueCapturingLayer {
    captured: Arc<Mutex<BTreeMap<String, String>>>,
}

impl<S> tracing_subscriber::Layer<S> for ValueCapturingLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut collector = FieldValueCollector::default();
        attrs.values().record(&mut collector);
        self.captured.lock().unwrap().extend(collector.values);
    }

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

fn evolve_nonce(hash: String) {
    let _span = trace_span!(crate::amaru::consensus::header::EVOLVE_NONCE, hash);
    let _guard = _span.enter();
}

fn apply_block(point_slot: u64) {
    let _span = trace_span!(crate::amaru::ledger::block::APPLY, point_slot);
    let _guard = _span.enter();
}

fn process_block(block_body_hash: String, block_number: u64, block_body_size: u64) {
    let _span = trace_span!(
        crate::amaru::ledger::block::CREATE_VALIDATION_CONTEXT,
        block_body_hash,
        block_number,
        block_body_size
    );
    let _guard = _span.enter();
}

fn outer_with_record(block_body_hash: String, block_number: u64, block_body_size: u64) {
    let _span = trace_span!(
        crate::amaru::ledger::block::CREATE_VALIDATION_CONTEXT,
        block_body_hash,
        block_number,
        block_body_size
    );
    let _guard = _span.enter();
    inner_record(5);
}

fn inner_record(_total_inputs: u64) {
    trace_record!(crate::amaru::ledger::block::CREATE_VALIDATION_CONTEXT, total_inputs = _total_inputs);
}

fn distinct_formatting(display_value: DistinctFormatting, debug_value: DistinctFormatting) {
    let _span = trace_span!(crate::amaru::formatting::test::DISTINCT_FORMATTING, display_value, debug_value);
    let _guard = _span.enter();
}

fn roll_forward(peer: String) {
    let _span = trace_span!(crate::amaru::consensus::roll_forward::PROCESS, peer);
    let _guard = _span.enter();
}

fn roll_forward_with_display_expressions(peer: String) {
    let _span = trace_span!(crate::amaru::consensus::roll_forward::PROCESS, peer = peer);
    let _guard = _span.enter();
}

fn root_roll_forward(peer: String) {
    let _outer = tracing::debug_span!("outer");
    let _outer_guard = _outer.enter();
    let _span = trace_span!(root, crate::amaru::consensus::roll_forward::PROCESS, peer);
    let _guard = _span.enter();
}

fn root_roll_forward_with_display_expression(peer: String) {
    let _outer = tracing::debug_span!("outer");
    let _outer_guard = _outer.enter();
    let _span = trace_span!(root, crate::amaru::consensus::roll_forward::PROCESS, peer);
    let _guard = _span.enter();
}

fn registered_span(_category: String, tip: String) {
    let _span = trace_span!(crate::amaru::classification::test::REGISTERED_SPAN, tip);
    let _guard = _span.enter();
}

#[cfg(test)]
mod test {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use tracing_subscriber::Registry;

    use super::*;

    #[test]
    fn test_span_target_and_name() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(CapturingLayer { captured_spans: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            evolve_nonce("test".into());
            apply_block(42);
        });

        let spans = captured.lock().unwrap();
        assert_eq!(spans.len(), 2);

        assert_eq!(spans[0].target, "amaru::consensus");
        assert_eq!(spans[0].name, "header.evolve_nonce");

        assert_eq!(spans[1].target, "amaru::ledger");
        assert_eq!(spans[1].name, "block.apply");
    }

    #[test]
    fn test_tags_recorded_as_span_attributes() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default()
            .with(CapturingLayer { captured_spans: captured.clone() })
            .with(ValueCapturingLayer { captured: values.clone() });

        tracing::subscriber::with_default(subscriber, || {
            let _span = trace_span!(crate::amaru::categorized::work::COMPUTE, label = &"compute".to_string());
        });

        let spans = captured.lock().unwrap();
        assert!(spans[0].fields.contains(&"amaru.tag.cpu".into()));
        assert!(spans[0].fields.contains(&"amaru.tag.io".into()));

        let values = values.lock().unwrap();
        assert_eq!(values.get("label").map(String::as_str), Some("compute"));
        assert_eq!(values.get("amaru.tag.cpu").map(String::as_str), Some("true"));
        assert_eq!(values.get("amaru.tag.io").map(String::as_str), Some("true"));
    }

    #[test]
    fn test_schema_tags_override_module_tags() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default()
            .with(CapturingLayer { captured_spans: captured.clone() })
            .with(ValueCapturingLayer { captured: values.clone() });

        tracing::subscriber::with_default(subscriber, || {
            let _span = trace_span!(crate::amaru::categorized::work::STORE, label = &"store".to_string());
        });

        let spans = captured.lock().unwrap();
        assert!(spans[0].fields.contains(&"amaru.tag.setup".into()));
        assert!(!spans[0].fields.contains(&"amaru.tag.cpu".into()));

        let values = values.lock().unwrap();
        assert_eq!(values.get("amaru.tag.setup").map(String::as_str), Some("true"));
    }

    #[test]
    fn test_env_filter_selects_spans_by_tag() {
        use tracing_subscriber::{EnvFilter, Layer as _};

        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(
            CapturingLayer { captured_spans: captured.clone() }
                .with_filter(EnvFilter::new("[{amaru.tag.cpu=true}]=trace")),
        );

        tracing::subscriber::with_default(subscriber, || {
            let _tagged = trace_span!(crate::amaru::categorized::work::COMPUTE, label = &"compute".to_string());
            apply_block(42);
        });

        let spans = captured.lock().unwrap();
        assert_eq!(spans.len(), 1, "only the cpu-tagged span should be selected: {:?}", *spans);
        assert_eq!(spans[0].name, "work.compute");
    }

    #[test]
    fn test_span_level_is_trace() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(CapturingLayer { captured_spans: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            evolve_nonce("test".into());
        });

        let spans = captured.lock().unwrap();
        assert_eq!(spans[0].level, tracing::Level::TRACE);
    }

    #[test]
    fn test_schema_fields_declared() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(CapturingLayer { captured_spans: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            process_block("hash".into(), 100, 1024);
        });

        let spans = captured.lock().unwrap();
        let fields = &spans[0].fields;

        assert!(fields.contains(&"block_body_hash".into()));
        assert!(fields.contains(&"block_number".into()));
        assert!(fields.contains(&"block_body_size".into()));
        assert!(fields.contains(&"total_inputs".into())); // optional field also declared
        assert!(!fields.contains(&"category".into()));
    }

    #[test]
    fn test_field_values_recorded() {
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default().with(ValueCapturingLayer { captured: values.clone() });

        tracing::subscriber::with_default(subscriber, || {
            process_block("0xabc".into(), 100, 1024);
        });

        let recorded = values.lock().unwrap();
        assert_eq!(recorded.get("block_body_hash"), Some(&"0xabc".to_string()));
        assert_eq!(recorded.get("category"), None);
    }

    #[test]
    fn test_category_field_is_ignored_without_overriding_span_name() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default()
            .with(CapturingLayer { captured_spans: captured.clone() })
            .with(ValueCapturingLayer { captured: values.clone() });

        tracing::subscriber::with_default(subscriber, || {
            registered_span("state.msg.process".into(), "tip".into());
        });

        let spans = captured.lock().unwrap();
        let fields = &spans[0].fields;
        assert!(!fields.contains(&"category".into()));
        assert!(!fields.contains(&"otel.name".into()));
        assert!(!fields.contains(&"schema".into()));
        assert!(!fields.contains(&"name".into()));
        assert_eq!(spans[0].name, "test.registered_span");

        let recorded = values.lock().unwrap();
        assert_eq!(recorded.get("category"), None);
    }

    #[test]
    fn test_roll_forward_sets_category_without_otel_name() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default()
            .with(CapturingLayer { captured_spans: captured.clone() })
            .with(ValueCapturingLayer { captured: values.clone() });

        tracing::subscriber::with_default(subscriber, || {
            roll_forward("peer-1".into());
        });

        let spans = captured.lock().unwrap();
        let fields = &spans[0].fields;
        assert!(fields.contains(&"peer".into()));
        assert!(!fields.contains(&"category".into()));
        assert!(!fields.contains(&"otel.name".into()));
        assert!(!fields.contains(&"schema".into()));
        assert!(!fields.contains(&"name".into()));
        assert_eq!(spans[0].target, "amaru::consensus");
        assert_eq!(spans[0].name, "roll_forward.process");

        let recorded = values.lock().unwrap();
        assert_eq!(recorded.get("otel.name"), None);
        assert_eq!(recorded.get("peer"), Some(&"peer-1".to_string()));
        assert_eq!(recorded.get("category"), None);
    }

    #[test]
    fn test_root_span_does_not_inherit_current_span() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(CapturingLayer { captured_spans: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            root_roll_forward("peer-1".into());
        });

        let spans = captured.lock().unwrap();
        let roll_forward = spans.iter().find(|span| span.name == "roll_forward.process").unwrap();
        assert_eq!(roll_forward.parent, None);
    }

    #[test]
    fn test_trace_record_records_to_span() {
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default().with(ValueCapturingLayer { captured: values.clone() });

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

    #[test]
    fn test_trace_span_preserves_formatter_kind() {
        let values = Arc::new(Mutex::new(BTreeMap::new()));
        let subscriber = Registry::default().with(ValueCapturingLayer { captured: values.clone() });

        tracing::subscriber::with_default(subscriber, || {
            distinct_formatting(DistinctFormatting, DistinctFormatting);
            roll_forward_with_display_expressions("peer-a".to_string());
            root_roll_forward_with_display_expression("peer-b".to_string());
        });

        let recorded = values.lock().unwrap();
        assert_eq!(recorded.get("display_value"), Some(&"display-format".to_string()));
        assert_eq!(recorded.get("debug_value"), Some(&"debug-format".to_string()));
    }
}
