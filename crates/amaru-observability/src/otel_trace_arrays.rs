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

//! Upgrade CBOR `record_bytes` fields on **spans** to classic OTEL attribute values.
//!
//! The stock `tracing-opentelemetry` layer has no `record_bytes` handler, so CBOR payloads
//! fall through to hex debug strings. This layer re-visits span attributes and:
//! - for homogeneous CBOR arrays / scalars, calls [`OpenTelemetrySpanExt::set_attribute`]
//!   with a proper [`Value::Array`](opentelemetry::Value::Array) or scalar `Value`;
//! - for maps, mixed arrays, and other non-representable shapes, sets a **CBOR diagnostic**
//!   string so operators see readable structure instead of hex.
//!
//! Place this layer **after** `tracing_opentelemetry::OpenTelemetryLayer` so the OTEL span
//! exists when attributes are applied. The stock layer has no `record_bytes` visitor, so it
//! also stores a debug dump of the CBOR bytes under the same key. [`CborSpanExporter`]
//! replaces that dump with the upgraded value and keeps one attribute per key.
//!
//! ## Lifecycle / never-entered spans
//!
//! Attributes collected at span creation are queued in span extensions and applied when the
//! span is **entered** (`on_enter`) or when a later `record` happens while the span is
//! current. There is intentionally **no** `on_close` flush: the public
//! [`OpenTelemetrySpanExt::set_attribute`] API requires a [`tracing::Span`] handle, which
//! layers do not have at close time (and `Span::current()` is usually the wrong span).
//!
//! **Spans that are created with CBOR fields but never entered** (for example parent-only
//! handles used solely as `parent: &span`) therefore keep the stock hex form for those
//! fields. Prefer entering/`in_scope`/`#[instrument]` when structured OTEL attributes matter.

use cbor_data::Cbor;
use opentelemetry::{Key, KeyValue, Value as TraceValue};
use tracing::field::{Field, Visit};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{Layer, registry::LookupSpan};

use crate::field::cbor_to_trace_value;

/// Pending homogeneous-array (and scalar) upgrades collected at span build time.
struct PendingTraceAttributes {
    attrs: Vec<(Key, TraceValue)>,
}

/// Layer that upgrades CBOR span fields to `Value::Array` / scalar `Value` when possible.
#[derive(Debug, Default, Clone, Copy)]
pub struct CborTraceArrayLayer;

impl CborTraceArrayLayer {
    pub fn new() -> Self {
        Self
    }
}

struct CollectTraceValues {
    attrs: Vec<(Key, TraceValue)>,
}

impl Visit for CollectTraceValues {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, _field: &Field, _value: &str) {}
    fn record_bool(&mut self, _field: &Field, _value: bool) {}
    fn record_i64(&mut self, _field: &Field, _value: i64) {}
    fn record_u64(&mut self, _field: &Field, _value: u64) {}
    fn record_f64(&mut self, _field: &Field, _value: f64) {}

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        // Always set an attribute: native Array/scalar when possible, else diagnostic text.
        self.attrs.push((Key::new(field.name()), cbor_to_trace_value(value)));
    }
}

fn apply_pending(span: &tracing::Span, pending: PendingTraceAttributes) {
    for (key, value) in pending.attrs {
        span.set_attribute(key, value);
    }
}

/// Keep the last value for each attribute key.
///
/// A CBOR field is visited twice: the stock layer records a debug dump of the bytes, then
/// [`CborTraceArrayLayer`] records the decoded value. Export must show the decoded value once.
pub fn retain_last_attribute(attributes: &mut Vec<KeyValue>) {
    let mut index = 0;
    while index < attributes.len() {
        if let Some(upgraded) = upgrade_debug_cbor(&attributes[index].value) {
            attributes[index].value = upgraded;
        }
        let key = attributes[index].key.clone();
        if attributes[index + 1..].iter().any(|later| later.key == key) {
            attributes.remove(index);
        } else {
            index += 1;
        }
    }
}

/// Replace a stock `record_bytes` debug dump (`[58 20 ab …]`) with the decoded CBOR value.
fn upgrade_debug_cbor(value: &TraceValue) -> Option<TraceValue> {
    let TraceValue::String(text) = value else {
        return None;
    };
    let text = text.as_str();
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;
    if inner.is_empty() {
        return None;
    }
    let mut bytes = Vec::new();
    for part in inner.split(' ') {
        if part.len() != 2 {
            return None;
        }
        bytes.push(u8::from_str_radix(part, 16).ok()?);
    }
    Cbor::checked(&bytes).ok()?;
    Some(cbor_to_trace_value(&bytes))
}

/// Replace CBOR debug dumps on a finished span, including attributes copied onto span events.
pub fn prepare_exported_attributes(attributes: &mut Vec<KeyValue>, events: &mut [opentelemetry::trace::Event]) {
    retain_last_attribute(attributes);
    for event in events {
        retain_last_attribute(&mut event.attributes);
    }
}

impl<S> Layer<S> for CborTraceArrayLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = CollectTraceValues { attrs: Vec::new() };
        attrs.record(&mut visitor);
        if visitor.attrs.is_empty() {
            return;
        }
        if let Some(span_ref) = ctx.span(id) {
            span_ref.extensions_mut().insert(PendingTraceAttributes { attrs: visitor.attrs });
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = CollectTraceValues { attrs: Vec::new() };
        values.record(&mut visitor);
        if visitor.attrs.is_empty() {
            return;
        }

        // If this span is current, apply immediately; otherwise queue until enter.
        let current = tracing::Span::current();
        if current.id().as_ref() == Some(id) {
            apply_pending(&current, PendingTraceAttributes { attrs: visitor.attrs });
            return;
        }
        if let Some(span_ref) = ctx.span(id) {
            let mut ext = span_ref.extensions_mut();
            if let Some(pending) = ext.get_mut::<PendingTraceAttributes>() {
                pending.attrs.extend(visitor.attrs);
            } else {
                ext.insert(PendingTraceAttributes { attrs: visitor.attrs });
            }
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span_ref) = ctx.span(id) else {
            return;
        };
        let pending = span_ref.extensions_mut().remove::<PendingTraceAttributes>();
        if let Some(pending) = pending {
            // Entering makes this the current span.
            apply_pending(&tracing::Span::current(), pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::{Array, Value as TraceValue};

    use crate::field::{cbor_to_trace_value, encode_cbor};

    #[test]
    fn homogeneous_string_array_becomes_trace_array() {
        let bytes = encode_cbor(&vec!["a:1".to_string(), "b:2".to_string()]);
        let value = cbor_to_trace_value(&bytes);
        let TraceValue::Array(Array::String(items)) = value else {
            panic!("expected string array, got {value:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), "a:1");
        assert_eq!(items[1].as_str(), "b:2");
    }

    #[test]
    fn homogeneous_i64_array_becomes_trace_array() {
        let bytes = encode_cbor(&vec![1_i64, 2, 3]);
        let value = cbor_to_trace_value(&bytes);
        let TraceValue::Array(Array::I64(items)) = value else {
            panic!("expected i64 array, got {value:?}");
        };
        assert_eq!(items, vec![1, 2, 3]);
    }

    #[test]
    fn homogeneous_bool_array_becomes_trace_array() {
        let bytes = encode_cbor(&vec![true, false, true]);
        let value = cbor_to_trace_value(&bytes);
        let TraceValue::Array(Array::Bool(items)) = value else {
            panic!("expected bool array, got {value:?}");
        };
        assert_eq!(items, vec![true, false, true]);
    }

    #[test]
    fn map_falls_back_to_diagnostic_string() {
        #[derive(serde::Serialize)]
        struct M {
            a: u64,
        }
        let bytes = encode_cbor(&M { a: 1 });
        let value = cbor_to_trace_value(&bytes);
        let TraceValue::String(s) = value else {
            panic!("expected diagnostic string for map, got {value:?}");
        };
        let text = s.as_str();
        assert!(text.contains('a') || text.contains('1'), "diagnostic={text}");
    }

    #[test]
    fn mixed_array_falls_back_to_diagnostic_string() {
        // Tuple serializes as a mixed CBOR array, not a classic homogeneous TraceValue array.
        let bytes = encode_cbor(&(1_u64, "x"));
        let value = cbor_to_trace_value(&bytes);
        let TraceValue::String(s) = value else {
            panic!("expected diagnostic string for mixed array, got {value:?}");
        };
        assert!(!s.as_str().is_empty());
    }
}
