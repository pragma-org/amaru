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

const TAG_FIELD_PREFIX: &str = "amaru.tag.";

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
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        self.emit(TelemetryRecord {
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            name: event.metadata().name().to_string(),
            at: Instant::now(),
            wall_time: SystemTime::now(),
            fields: visitor.fields,
            duration: None,
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

        let mut extensions = span.extensions_mut();
        let Some(state) = extensions.remove::<CapturedSpan>() else {
            return;
        };

        let closed_at = Instant::now();
        self.emit(TelemetryRecord {
            level: state.level,
            target: state.target,
            name: state.name,
            at: closed_at,
            wall_time: state.wall_time,
            fields: state.fields,
            duration: Some(closed_at.saturating_duration_since(state.opened_at)),
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
