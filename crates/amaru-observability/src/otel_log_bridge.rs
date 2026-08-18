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

//! Project-owned `tracing` → OpenTelemetry **logs** bridge.
//!
//! Based on the stock `opentelemetry-appender-tracing` bridge, but extended so that
//! Amaru's CBOR `record_bytes` payloads become nested [`AnyValue`] maps/lists instead
//! of opaque [`AnyValue::Bytes`].
//!
//! ## Why a project-owned bridge?
//!
//! This module vendors a focused subset of the stock tracing → OTEL logs bridge so that:
//!
//! - we own the field transport contract (typed primitives + CBOR for complex values);
//! - CBOR `record_bytes` become nested `AnyValue` maps/lists rather than opaque Bytes
//!   (upstream still treats them as Bytes and has open TODOs for richer shapes);
//! - Visit → `AnyValue` can evolve without coupling to `opentelemetry-appender-tracing` releases.
//!
//! Experimental stock features (log-crate metadata attributes, copying tracing-span fields
//! onto log records) are intentionally omitted and can be added later if needed.

use opentelemetry::{
    Key,
    logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity},
    trace::TraceContextExt,
};
use tracing::Level;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::Layer;

use crate::field::cbor_to_any_value;

/// `tracing` → OTEL logs bridge with CBOR-aware structured attributes.
///
/// Drop-in alternative to `opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge`
/// for Amaru (and embedders that emit Amaru-style structured fields).
pub struct CborOtelLogBridge<P, L>
where
    P: LoggerProvider<Logger = L> + Send + Sync,
    L: Logger + Send + Sync,
{
    logger: L,
    _phantom: std::marker::PhantomData<P>,
}

impl<P, L> CborOtelLogBridge<P, L>
where
    P: LoggerProvider<Logger = L> + Send + Sync,
    L: Logger + Send + Sync,
{
    /// Create a bridge using the given logger provider (empty instrumentation scope name,
    /// matching the stock appender-tracing default).
    pub fn new(provider: &P) -> Self {
        Self { logger: provider.logger(""), _phantom: std::marker::PhantomData }
    }
}

struct EventVisitor<'a, LR: LogRecord> {
    log_record: &'a mut LR,
}

impl<'a, LR: LogRecord> EventVisitor<'a, LR> {
    fn new(log_record: &'a mut LR) -> Self {
        Self { log_record }
    }

    fn add_attr(&mut self, name: &'static str, value: AnyValue) {
        self.log_record.add_attribute(Key::new(name), value);
    }
}

impl<LR: LogRecord> tracing::field::Visit for EventVisitor<'_, LR> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.log_record.set_body(format!("{value:?}").into());
        } else {
            self.add_attr(field.name(), AnyValue::from(format!("{value:?}")));
        }
    }

    fn record_error(&mut self, _field: &tracing::field::Field, value: &(dyn std::error::Error + 'static)) {
        self.add_attr("exception.message", AnyValue::from(value.to_string()));
    }

    fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
        // Project contract: every record_bytes payload is CBOR → nested AnyValue.
        self.add_attr(field.name(), cbor_to_any_value(value));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.log_record.set_body(AnyValue::from(value.to_owned()));
        } else {
            self.add_attr(field.name(), AnyValue::from(value.to_owned()));
        }
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.add_attr(field.name(), AnyValue::from(value));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.add_attr(field.name(), AnyValue::from(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.add_attr(field.name(), AnyValue::from(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if let Ok(signed) = i64::try_from(value) {
            self.add_attr(field.name(), AnyValue::from(signed));
        } else {
            self.add_attr(field.name(), AnyValue::from(value.to_string()));
        }
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        if let Ok(signed) = i64::try_from(value) {
            self.add_attr(field.name(), AnyValue::from(signed));
        } else {
            self.add_attr(field.name(), AnyValue::from(value.to_string()));
        }
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        if let Ok(signed) = i64::try_from(value) {
            self.add_attr(field.name(), AnyValue::from(signed));
        } else {
            self.add_attr(field.name(), AnyValue::from(value.to_string()));
        }
    }
}

impl<S, P, L> Layer<S> for CborOtelLogBridge<P, L>
where
    S: tracing::Subscriber,
    P: LoggerProvider<Logger = L> + Send + Sync + 'static,
    L: Logger + Send + Sync + 'static,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let metadata = event.metadata();
        let severity = severity_of_level(metadata.level());
        let target = metadata.target();
        let name = metadata.name();
        if !self.logger.event_enabled(severity, target, Some(name)) {
            return;
        }

        let mut log_record = self.logger.create_log_record();
        log_record.set_target(target);
        log_record.set_event_name(name);
        log_record.set_severity_number(severity);
        log_record.set_severity_text(metadata.level().as_str());

        let mut visitor = EventVisitor::new(&mut log_record);
        event.record(&mut visitor);

        // Associate the log with the current OTEL span when one is entered, so
        // Loki/Tempo (and similar) can join logs to the same nested span tree
        // that traces already form natively.
        let span_context = tracing::Span::current().context().span().span_context().clone();
        if span_context.is_valid() {
            log_record.set_trace_context(
                span_context.trace_id(),
                span_context.span_id(),
                Some(span_context.trace_flags()),
            );
        }

        self.logger.emit(log_record);
    }
}

const fn severity_of_level(level: &Level) -> Severity {
    match *level {
        Level::TRACE => Severity::Trace,
        Level::DEBUG => Severity::Debug,
        Level::INFO => Severity::Info,
        Level::WARN => Severity::Warn,
        Level::ERROR => Severity::Error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use opentelemetry::{Key, logs::AnyValue};
    use serde::Serialize;
    use tracing_subscriber::{Layer, layer::SubscriberExt};

    use super::*;
    use crate::field::encode_cbor;

    #[derive(Serialize)]
    struct Sample {
        peers: Vec<String>,
        count: u64,
        ok: bool,
    }

    #[derive(Default)]
    struct CaptureRecord {
        attrs: Vec<(String, AnyValue)>,
    }

    impl LogRecord for CaptureRecord {
        fn set_event_name(&mut self, _name: &'static str) {}
        fn set_target<T>(&mut self, _target: T)
        where
            T: Into<std::borrow::Cow<'static, str>>,
        {
        }
        fn set_timestamp(&mut self, _timestamp: std::time::SystemTime) {}
        fn set_observed_timestamp(&mut self, _timestamp: std::time::SystemTime) {}
        fn set_severity_text(&mut self, _text: &'static str) {}
        fn set_severity_number(&mut self, _number: Severity) {}
        fn set_body(&mut self, _body: AnyValue) {}
        fn add_attributes<I, K, V>(&mut self, attributes: I)
        where
            I: IntoIterator<Item = (K, V)>,
            K: Into<Key>,
            V: Into<AnyValue>,
        {
            for (k, v) in attributes {
                self.attrs.push((k.into().as_str().to_owned(), v.into()));
            }
        }
        fn add_attribute<K, V>(&mut self, key: K, value: V)
        where
            K: Into<Key>,
            V: Into<AnyValue>,
        {
            self.attrs.push((key.into().as_str().to_owned(), value.into()));
        }
    }

    struct CaptureLayer {
        out: Arc<Mutex<Vec<(String, AnyValue)>>>,
    }

    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            let mut rec = CaptureRecord::default();
            let mut visitor = EventVisitor::new(&mut rec);
            event.record(&mut visitor);
            *self.out.lock().unwrap() = rec.attrs;
        }
    }

    #[test]
    fn visitor_decodes_cbor_bytes_to_map_with_list() {
        let sample = Sample { peers: vec!["a:1".into(), "b:2".into()], count: 7, ok: true };
        let cbor = encode_cbor(&sample);
        let out = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer { out: out.clone() });

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(payload = cbor.as_ref() as &[u8], "test");
        });

        let attrs = out.lock().unwrap().clone();
        let payload = attrs.into_iter().find(|(k, _)| k == "payload").expect("payload attr").1;

        let AnyValue::Map(map) = payload else {
            panic!("expected Map for payload");
        };
        assert!(matches!(map.get(&Key::new("count")), Some(AnyValue::Int(7))));
        assert!(matches!(map.get(&Key::new("ok")), Some(AnyValue::Boolean(true))));
        let Some(AnyValue::ListAny(list)) = map.get(&Key::new("peers")) else {
            panic!("expected ListAny for peers");
        };
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn visitor_preserves_typed_primitives() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer { out: out.clone() });

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(git_dirty = true, version = "10.11.0", n = 42_u64, "build");
        });

        let attrs: std::collections::BTreeMap<_, _> = out.lock().unwrap().clone().into_iter().collect();
        assert!(matches!(attrs.get("git_dirty"), Some(AnyValue::Boolean(true))));
        assert!(matches!(attrs.get("version"), Some(AnyValue::String(_))));
        assert!(matches!(attrs.get("n"), Some(AnyValue::Int(42))));
    }
}
