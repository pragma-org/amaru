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

//! Tests for trace_span! and trace_record! macros with custom tracing levels

use amaru_observability_macros::{define_local_schemas, trace_record, trace_span};
use std::sync::{Arc, Mutex};
use tracing::field::Visit;
use tracing_subscriber::{Registry, layer::SubscriberExt};

define_local_schemas! {
    database {
        operations {
            /// Database query operation
            QUERY {
                required query_id: u64
                optional table_name: String
            }
        }
    }
}

#[derive(Default, Clone)]
struct CapturedSpan {
    level: String,
}

#[derive(Default, Clone)]
struct CapturedEvent {
    level: String,
    fields: std::collections::BTreeMap<String, String>,
}

#[derive(Default)]
struct CaptureLayer {
    spans: Arc<Mutex<Vec<CapturedSpan>>>,
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = format!("{}", attrs.metadata().level());
        let capture = CapturedSpan { level };
        self.spans.lock().unwrap().push(capture);
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = format!("{}", event.metadata().level());

        let mut fields = std::collections::BTreeMap::new();
        let mut visitor = EventFieldVisitor(&mut fields);
        event.record(&mut visitor);

        let capture = CapturedEvent { level, fields };

        self.events.lock().unwrap().push(capture);
    }
}

struct EventFieldVisitor<'a>(&'a mut std::collections::BTreeMap<String, String>);

impl<'a> Visit for EventFieldVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{:?}", value));
    }
}

// ============================================================================
// trace_span! tests
// ============================================================================

#[test]
fn test_trace_span_default_trace_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: spans.clone(),
        events: Arc::new(Mutex::new(Vec::new())),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = trace_span!(database::operations::QUERY, query_id = 12345);
    });

    let captured = spans.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].level, "TRACE");
}

#[test]
fn test_trace_span_with_debug_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: spans.clone(),
        events: Arc::new(Mutex::new(Vec::new())),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = trace_span!(DEBUG, database::operations::QUERY, query_id = 12345);
    });

    let captured = spans.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].level, "DEBUG");
}

#[test]
fn test_trace_span_with_info_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: spans.clone(),
        events: Arc::new(Mutex::new(Vec::new())),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = trace_span!(INFO, database::operations::QUERY, query_id = 99);
    });

    let captured = spans.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].level, "INFO");
}

#[test]
fn test_trace_span_with_warn_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: spans.clone(),
        events: Arc::new(Mutex::new(Vec::new())),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = trace_span!(WARN, database::operations::QUERY, query_id = 42);
    });

    let captured = spans.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].level, "WARN");
}

#[test]
fn test_trace_span_with_error_level() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: spans.clone(),
        events: Arc::new(Mutex::new(Vec::new())),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = trace_span!(ERROR, database::operations::QUERY, query_id = 1);
    });

    let captured = spans.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].level, "ERROR");
}

// ============================================================================
// trace_record! tests
// ============================================================================

#[test]
fn test_trace_record_without_level_no_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: Arc::new(Mutex::new(Vec::new())),
        events: events.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("test").entered();
        trace_record!(database::operations::QUERY, query_id = 12345);
    });

    let captured = events.lock().unwrap();
    assert_eq!(
        captured.len(),
        0,
        "No event should be emitted without level"
    );
}

#[test]
fn test_trace_record_with_debug_level_emits_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: Arc::new(Mutex::new(Vec::new())),
        events: events.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("test").entered();
        trace_record!(DEBUG, database::operations::QUERY, query_id = 555);
    });

    let captured = events.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "Event should be emitted with DEBUG level"
    );
    assert_eq!(captured[0].level, "DEBUG");
    assert!(captured[0].fields.contains_key("query_id"));
}

#[test]
fn test_trace_record_with_info_level_emits_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: Arc::new(Mutex::new(Vec::new())),
        events: events.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("test").entered();
        trace_record!(INFO, database::operations::QUERY, query_id = 777);
    });

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1, "Event should be emitted with INFO level");
    assert_eq!(captured[0].level, "INFO");
}

#[test]
fn test_trace_record_with_warn_level_emits_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: Arc::new(Mutex::new(Vec::new())),
        events: events.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("test").entered();
        trace_record!(WARN, database::operations::QUERY, query_id = 888);
    });

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1, "Event should be emitted with WARN level");
    assert_eq!(captured[0].level, "WARN");
}

#[test]
fn test_trace_record_with_error_level_emits_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: Arc::new(Mutex::new(Vec::new())),
        events: events.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("test").entered();
        trace_record!(ERROR, database::operations::QUERY, query_id = 999);
    });

    let captured = events.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "Event should be emitted with ERROR level"
    );
    assert_eq!(captured[0].level, "ERROR");
}

#[test]
fn test_trace_record_with_trace_level_emits_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer {
        spans: Arc::new(Mutex::new(Vec::new())),
        events: events.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("test").entered();
        trace_record!(TRACE, database::operations::QUERY, query_id = 111);
    });

    let captured = events.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "Event should be emitted with TRACE level"
    );
    assert_eq!(captured[0].level, "TRACE");
}
