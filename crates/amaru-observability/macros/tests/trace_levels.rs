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

//! Tests for the #[trace] macro with custom tracing levels
//!
//! These tests verify that the #[trace] macro correctly supports
//! different tracing levels (trace, debug, info, warn, error)

use amaru_observability_macros::{define_local_schemas, trace};
use std::sync::{Arc, Mutex};
use tracing::field::Visit;
use tracing_subscriber::{Registry, layer::SubscriberExt};

define_local_schemas! {
    network {
        sync {
            /// Network sync events
            SYNC_BLOCKS {
                required block_height: u64
            }

            /// Connection events
            CONNECTION_OPENED {
                required peer_id: String
                optional ip_address: String
            }
        }
    }

    validation {
        rules {
            /// Rule validation events
            VALIDATE_RULE {
                required rule_name: String
                required result: String
            }
        }
    }
}

#[derive(Default)]
struct SpanCapture {
    name: String,
    target: String,
    level: String,
    fields: std::collections::BTreeMap<String, String>,
}

#[derive(Default)]
struct SpanCaptureLayer {
    spans: Arc<Mutex<Vec<SpanCapture>>>,
}

impl<S> tracing_subscriber::Layer<S> for SpanCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Record the level from the metadata
        let level = format!("{}", attrs.metadata().level());

        let mut capture = SpanCapture {
            name: String::new(),
            target: String::new(),
            level,
            fields: Default::default(),
        };

        let mut visitor = SpanMetadataVisitor(&mut capture);
        attrs.record(&mut visitor);

        self.spans.lock().unwrap().push(capture);
    }
}

struct SpanMetadataVisitor<'a>(&'a mut SpanCapture);

impl<'a> Visit for SpanMetadataVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let field_name = field.name();
        let value_str = format!("{:?}", value);

        // Capture metadata fields
        if field_name == "span.name" {
            self.0.name = value_str;
        } else if field_name == "span.target" {
            self.0.target = value_str;
        } else if field_name == "span.level" {
            self.0.level = value_str;
        } else {
            self.0.fields.insert(field_name.to_string(), value_str);
        }
    }
}

// ============================================================================
// Test functions with different levels
// ============================================================================

#[trace(network::sync::SYNC_BLOCKS)]
fn sync_blocks_trace(block_height: u64) {
    let _ = block_height;
}

#[trace(DEBUG, network::sync::SYNC_BLOCKS)]
fn sync_blocks_debug(block_height: u64) {
    let _ = block_height;
}

#[trace(INFO, network::sync::CONNECTION_OPENED)]
fn connection_opened_info(peer_id: String) {
    let _ = peer_id;
}

#[trace(WARN, validation::rules::VALIDATE_RULE)]
fn validate_rule_warn(rule_name: String, result: String) {
    let _ = (rule_name, result);
}

#[trace(ERROR, validation::rules::VALIDATE_RULE)]
fn validate_rule_error(rule_name: String, result: String) {
    let _ = (rule_name, result);
}

// With custom field expressions and level
#[trace(DEBUG, network::sync::CONNECTION_OPENED, ip_address = "127.0.0.1")]
fn connection_opened_with_ip(peer_id: String) {
    let _ = peer_id;
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn test_trace_with_debug_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(SpanCaptureLayer {
        spans: spans.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        sync_blocks_debug(12345);
    });

    let captured_spans = spans.lock().unwrap();
    assert!(!captured_spans.is_empty(), "Expected a span to be captured");

    let span = &captured_spans[0];
    assert_eq!(
        span.level, "DEBUG",
        "Expected DEBUG level, got {}",
        span.level
    );
}

#[test]
fn test_trace_with_info_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(SpanCaptureLayer {
        spans: spans.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        connection_opened_info("peer-123".into());
    });

    let captured_spans = spans.lock().unwrap();
    assert!(!captured_spans.is_empty(), "Expected a span to be captured");

    let span = &captured_spans[0];
    assert_eq!(
        span.level, "INFO",
        "Expected INFO level, got {}",
        span.level
    );
}

#[test]
fn test_trace_with_warn_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(SpanCaptureLayer {
        spans: spans.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        validate_rule_warn("firewall".into(), "rejected".into());
    });

    let captured_spans = spans.lock().unwrap();
    assert!(!captured_spans.is_empty(), "Expected a span to be captured");

    let span = &captured_spans[0];
    assert_eq!(
        span.level, "WARN",
        "Expected WARN level, got {}",
        span.level
    );
}

#[test]
fn test_trace_with_error_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(SpanCaptureLayer {
        spans: spans.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        validate_rule_error("critical".into(), "failed".into());
    });

    let captured_spans = spans.lock().unwrap();
    assert!(!captured_spans.is_empty(), "Expected a span to be captured");

    let span = &captured_spans[0];
    assert_eq!(
        span.level, "ERROR",
        "Expected ERROR level, got {}",
        span.level
    );
}

#[test]
fn test_trace_default_trace_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(SpanCaptureLayer {
        spans: spans.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        sync_blocks_trace(54321);
    });

    let captured_spans = spans.lock().unwrap();
    assert!(!captured_spans.is_empty(), "Expected a span to be captured");

    let span = &captured_spans[0];
    assert_eq!(
        span.level, "TRACE",
        "Expected TRACE level (default), got {}",
        span.level
    );
}

#[test]
fn test_trace_with_level_and_custom_fields() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(SpanCaptureLayer {
        spans: spans.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        connection_opened_with_ip("peer-456".into());
    });

    let captured_spans = spans.lock().unwrap();
    assert!(!captured_spans.is_empty(), "Expected a span to be captured");

    let span = &captured_spans[0];
    assert_eq!(
        span.level, "DEBUG",
        "Expected DEBUG level with custom fields"
    );
}
