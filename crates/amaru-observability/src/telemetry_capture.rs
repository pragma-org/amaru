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

//! Shared capture of structured tracing events for the TUI and embedders.
//!
//! Events are delivered as target / name / fields maps so subscribers can match
//! against the typed tracing schemas re-exported from this crate without depending
//! on log string formatting.

use std::{
    collections::BTreeMap,
    fmt,
    sync::mpsc::{SyncSender, TrySendError},
    time::{Instant, SystemTime},
};

use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::{
    field::{DecodedField, TAG_FIELD_PREFIX, cbor_to_decoded_field},
    span_encode::ancestor_span_names,
};

/// A captured field value from a tracing event or span.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Debug(String),
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(v) => write!(f, "{v}"),
            Self::I64(v) => write!(f, "{v}"),
            Self::U64(v) => write!(f, "{v}"),
            Self::F64(v) => write!(f, "{v}"),
            Self::Str(v) | Self::Debug(v) => write!(f, "{v}"),
        }
    }
}

/// One telemetry record (event or closed span).
#[derive(Debug, Clone)]
pub struct TelemetryRecord {
    pub level: Level,
    pub target: String,
    pub name: String,
    pub at: Instant,
    pub wall_time: SystemTime,
    pub fields: BTreeMap<String, FieldValue>,
    /// `None` for point events; `Some` when a span closed.
    pub duration: Option<std::time::Duration>,
    /// Ancestor span names, outermost first, excluding the wrapping span.
    pub parents: Vec<String>,
    /// Name of the wrapping span (`None` when the record is outside any span).
    pub span_name: Option<String>,
    /// Present on closed-span records (the span's own id).
    pub id: Option<u64>,
    /// Id of the parent of the wrapping span, when that parent exists.
    pub parent_id: Option<u64>,
}

/// `tracing-subscriber` layer that forwards events and closed spans to a channel.
#[derive(Debug, Clone)]
pub struct TelemetryCaptureLayer {
    tx: SyncSender<TelemetryRecord>,
}

impl TelemetryCaptureLayer {
    pub fn new(tx: SyncSender<TelemetryRecord>) -> Self {
        Self { tx }
    }

    fn emit(&self, record: TelemetryRecord) {
        match self.tx.try_send(record) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

#[derive(Debug, Clone)]
struct CapturedSpan {
    level: Level,
    target: String,
    name: String,
    wall_time: SystemTime,
    opened_at: Instant,
    fields: BTreeMap<String, FieldValue>,
}

impl<S> Layer<S> for TelemetryCaptureLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let mut parents = Vec::new();
        let mut span_name = None;
        let mut parent_id = None;
        if let Some(scope) = ctx.event_scope(event) {
            let spans: Vec<_> = scope.from_root().collect();
            parents.extend(ancestor_span_names(spans.iter().map(|span| span.name().to_string())));
            if let Some(leaf) = spans.last() {
                span_name = Some(leaf.name().to_string());
                parent_id = leaf.parent().map(|parent| parent.id().into_u64());
                if let Some(state) = leaf.extensions().get::<CapturedSpan>() {
                    for (key, value) in &state.fields {
                        visitor.fields.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }

        self.emit(TelemetryRecord {
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            name: event.metadata().name().to_string(),
            at: Instant::now(),
            wall_time: SystemTime::now(),
            fields: visitor.fields,
            duration: None,
            parents,
            span_name,
            id: None,
            parent_id,
        });
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);

        span.extensions_mut().insert(CapturedSpan {
            level: *attrs.metadata().level(),
            target: attrs.metadata().target().to_string(),
            name: attrs.metadata().name().to_string(),
            wall_time: SystemTime::now(),
            opened_at: Instant::now(),
            fields: visitor.fields,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut visitor = FieldVisitor::default();
        values.record(&mut visitor);

        if let Some(state) = span.extensions_mut().get_mut::<CapturedSpan>() {
            state.fields.extend(visitor.fields);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };

        let parents =
            ancestor_span_names(span.scope().from_root().map(|ancestor| ancestor.name().to_string())).collect();
        let parent_id = span.parent().map(|parent| parent.id().into_u64());
        let id = span.id().into_u64();
        let Some(state) = span.extensions_mut().remove::<CapturedSpan>() else {
            return;
        };

        let closed_at = Instant::now();
        self.emit(TelemetryRecord {
            level: state.level,
            target: state.target,
            span_name: Some(state.name.clone()),
            name: state.name,
            at: closed_at,
            wall_time: state.wall_time,
            fields: state.fields,
            duration: Some(closed_at.saturating_duration_since(state.opened_at)),
            parents,
            id: Some(id),
            parent_id,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, FieldValue>,
}

impl Visit for FieldVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        self.fields.insert(field.name().to_owned(), FieldValue::F64(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        self.fields.insert(field.name().to_owned(), FieldValue::I64(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        self.fields.insert(field.name().to_owned(), FieldValue::U64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        self.fields.insert(field.name().to_owned(), FieldValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        self.fields.insert(field.name().to_owned(), FieldValue::Str(value.to_owned()));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        // Schema macros encode non-primitive fields as CBOR (`record_bytes`). Decode so the
        // TUI keeps typed values (e.g. Slot → U64) instead of the stock Debug byte dump.
        let decoded = match cbor_to_decoded_field(value) {
            DecodedField::Bool(b) => FieldValue::Bool(b),
            DecodedField::I64(i) => FieldValue::I64(i),
            DecodedField::U64(u) => FieldValue::U64(u),
            DecodedField::F64(f) => FieldValue::F64(f),
            DecodedField::Text(s) => FieldValue::Str(s),
        };
        self.fields.insert(field.name().to_owned(), decoded);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }
        self.fields.insert(field.name().to_owned(), FieldValue::Debug(format!("{value:?}")));
    }
}

/// Create a bounded channel and a layer that publishes into it.
pub fn subscribe_telemetry(capacity: usize) -> (TelemetryCaptureLayer, std::sync::mpsc::Receiver<TelemetryRecord>) {
    let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
    (TelemetryCaptureLayer::new(tx), rx)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use tracing_subscriber::prelude::*;

    use super::*;
    use crate::field::encode_cbor;

    #[test]
    fn record_bytes_decodes_cbor_scalars_not_raw_byte_debug() {
        let (tx, rx) = sync_channel(1);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            // Newtype-style u64 (same wire shape as Slot/Epoch) and a string via CBOR.
            let slot = encode_cbor(&42u64);
            let label = encode_cbor(&"tip");
            tracing::info!(slot = slot.as_ref() as &[u8], label = label.as_ref() as &[u8], "update");
        });

        let record = rx.recv().expect("event");
        assert_eq!(record.fields.get("slot"), Some(&FieldValue::U64(42)));
        assert_eq!(record.fields.get("label"), Some(&FieldValue::Str("tip".into())));
        // Must not look like Debug of raw bytes: `[1, 2, 3, …]`.
        if let Some(FieldValue::Debug(s) | FieldValue::Str(s)) = record.fields.get("slot") {
            panic!("slot should be typed U64, got string-like {s:?}");
        }
        assert!(record.parents.is_empty(), "point event outside a span has an empty path");
        assert!(record.span_name.is_none());
        assert!(record.parent_id.is_none());
        assert!(record.id.is_none());
    }

    #[test]
    fn events_inside_nested_spans_record_the_name_path() {
        let (tx, rx) = sync_channel(4);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            let outer = tracing::info_span!("epoch.transition");
            let _outer = outer.enter();
            let inner = tracing::info_span!("governance.ratify_proposals");
            let _inner = inner.enter();
            tracing::info!("ratification.summarize");
        });

        let records: Vec<_> = rx.try_iter().collect();
        let event = records
            .iter()
            .find(|r| r.duration.is_none() && !r.parents.is_empty())
            .unwrap_or_else(|| panic!("event, got {records:#?}"));
        assert_eq!(event.parents, vec!["epoch.transition".to_string()]);
        assert_eq!(event.span_name.as_deref(), Some("governance.ratify_proposals"));
        assert!(event.parent_id.is_some(), "child event refers to the outer span by id");
        assert!(event.id.is_none());
        let closed_inner = records
            .iter()
            .find(|r| r.name == "governance.ratify_proposals" && r.duration.is_some())
            .expect("inner close");
        assert_eq!(closed_inner.parents, vec!["epoch.transition".to_string()]);
        assert_eq!(closed_inner.span_name.as_deref(), Some("governance.ratify_proposals"));
        assert_eq!(closed_inner.parent_id, event.parent_id);
        assert!(closed_inner.id.is_some());
    }

    #[test]
    fn record_bytes_complex_values_use_diagnostic_text() {
        let (tx, rx) = sync_channel(1);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        let peers = encode_cbor(&vec!["a:1".to_string(), "b:2".to_string()]);
        let raw_debug = format!("{:?}", peers.as_ref());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(peers = peers.as_ref() as &[u8], "hello");
        });

        let record = rx.recv().expect("event");
        match record.fields.get("peers") {
            Some(FieldValue::Str(s)) => {
                assert!(s.contains("a:1"), "expected diagnostic with element text, got {s}");
                assert_ne!(s, &raw_debug, "must not be stock Debug of raw CBOR bytes");
            }
            other => panic!("expected Str diagnostic for peers array, got {other:?}"),
        }
    }
}
