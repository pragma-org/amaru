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

//! Record header-span durations when the tracing span closes.
//!
//! Place [`SpanDurationLayer`] outside the OpenTelemetry layer. `on_close` then runs after
//! that layer ends the exported span. [`offer_span_elapsed`] supplies that elapsed time;
//! without an offer the layer uses its own start-to-exit clock.

use std::{cell::RefCell, time::SystemTime};

use opentelemetry::Value as TraceValue;
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, registry::LookupSpan};

use crate::{amaru::network::perf::header::FORWARD, cbor_to_trace_value, note_span_duration};

thread_local! {
    static OFFERED_ELAPSED: RefCell<Option<(String, u64)>> = const { RefCell::new(None) };
}

/// Elapsed microseconds of the span that is closing on this thread.
///
/// The OpenTelemetry span processor calls this from `on_end`, which runs inside the
/// tracing `on_close` and before [`SpanDurationLayer`] records the value.
pub fn offer_span_elapsed(name: &str, micros: u64) {
    OFFERED_ELAPSED.with(|slot| *slot.borrow_mut() = Some((name.to_string(), micros)));
}

fn take_offered_elapsed() -> Option<(String, u64)> {
    OFFERED_ELAPSED.with(|slot| slot.borrow_mut().take())
}

struct TimedHeader {
    start: SystemTime,
    end: Option<SystemTime>,
    header_hash: String,
}

/// Records network `perf` span durations at close: span name and hex `header_hash` to microseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpanDurationLayer;

impl SpanDurationLayer {
    pub fn new() -> Self {
        Self
    }
}

struct HashVisit {
    header_hash: Option<String>,
}

impl Visit for HashVisit {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, _field: &Field, _value: &str) {}
    fn record_bool(&mut self, _field: &Field, _value: bool) {}
    fn record_i64(&mut self, _field: &Field, _value: i64) {}
    fn record_u64(&mut self, _field: &Field, _value: u64) {}
    fn record_f64(&mut self, _field: &Field, _value: f64) {}

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        if field.name() != "header_hash" {
            return;
        }
        let TraceValue::String(text) = cbor_to_trace_value(value) else {
            return;
        };
        let text = text.as_str();
        if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            self.header_hash = Some(text.to_string());
        }
    }
}

fn read_hash(record: impl FnOnce(&mut HashVisit)) -> Option<String> {
    let mut visit = HashVisit { header_hash: None };
    record(&mut visit);
    visit.header_hash
}

impl<S> Layer<S> for SpanDurationLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if attrs.metadata().target() != FORWARD::TARGET {
            return;
        }
        let Some(span) = ctx.span(id) else {
            return;
        };
        let header_hash = read_hash(|visit| attrs.record(visit));
        let mut extensions = span.extensions_mut();
        let Some(header_hash) = header_hash else {
            return;
        };
        extensions.insert(TimedHeader { start: SystemTime::now(), end: None, header_hash });
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if span.metadata().target() != FORWARD::TARGET {
            return;
        }
        let Some(header_hash) = read_hash(|visit| values.record(visit)) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(timed) = extensions.get_mut::<TimedHeader>() {
            timed.header_hash = header_hash;
        } else {
            extensions.insert(TimedHeader { start: SystemTime::now(), end: None, header_hash });
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if let Some(timed) = span.extensions_mut().get_mut::<TimedHeader>() {
            timed.end = Some(SystemTime::now());
        }
    }

    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let offered = take_offered_elapsed();
        let Some(span) = ctx.span(&id) else {
            return;
        };
        if span.metadata().target() != FORWARD::TARGET {
            return;
        }
        let name = span.name().to_string();
        let extensions = span.extensions();
        let Some(timed) = extensions.get::<TimedHeader>() else {
            return;
        };
        let own = timed.end.unwrap_or_else(SystemTime::now).duration_since(timed.start).unwrap_or_default().as_micros()
            as u64;
        let micros = offered.filter(|(offered_name, _)| offered_name == &name).map(|(_, micros)| micros).unwrap_or(own);
        note_span_duration(&name, &timed.header_hash, micros);
    }
}
