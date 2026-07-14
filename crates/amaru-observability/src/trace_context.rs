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

use std::{fmt::Display, str::FromStr};

use opentelemetry::{
    Context, ContextGuard,
    trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Parent context for spans that must cross stage boundaries.
/// It encapsulates an OpenTelemetry `SpanContext` to provide a few helper functions.
///
/// Serialization carries the `SpanContext` and whether a live span was attached: the
/// `tracing::Span` handle itself cannot be serialized, so deserialization substitutes a
/// disabled placeholder span (`tracing::Span::none()`). A deserialized `TraceContext` thus
/// keeps the same shape and can still parent new spans, but `record` and `close` have no
/// observable effect.
#[derive(Clone, Debug)]
pub struct TraceContext {
    /// We keep the span as an option in order to drop it when it's closed
    span: Option<tracing::Span>,
    span_context: SpanContext,
}

impl PartialEq for TraceContext {
    fn eq(&self, other: &Self) -> bool {
        self.span_context == other.span_context
    }
}

impl Eq for TraceContext {}

impl TraceContext {
    pub fn none() -> Self {
        Self { span: None, span_context: SpanContext::empty_context() }
    }

    pub fn context(&self) -> Context {
        if self.span_context.is_valid() {
            Context::new().with_remote_span_context(self.span_context.clone())
        } else {
            Context::new()
        }
    }

    pub fn attach(&self) -> ContextGuard {
        self.context().attach()
    }

    /// Close the corresponding span
    pub fn close(&mut self) {
        if let Some(span) = self.span.take() {
            drop(span)
        }
    }

    /// Append a field to the corresponding span
    pub fn record(&self, name: &'static str, value: &dyn Display) {
        if let Some(span) = &self.span {
            span.record(name, tracing::field::display(value));
        }
    }
}

impl From<tracing::Span> for TraceContext {
    fn from(span: tracing::Span) -> Self {
        let span_context = span.context().span().span_context().clone();
        Self { span: Some(span), span_context }
    }
}

impl From<&tracing::Span> for TraceContext {
    fn from(span: &tracing::Span) -> Self {
        let span_context = span.context().span().span_context().clone();
        Self { span: None, span_context }
    }
}

impl Default for TraceContext {
    fn default() -> Self {
        Self::none()
    }
}

/// Serializable representation of a `TraceContext`: the OpenTelemetry `SpanContext` fields
/// plus whether a live span was attached.
#[derive(Serialize, Deserialize)]
struct SerializedTraceContext {
    has_span: bool,
    trace_id: String,
    span_id: String,
    trace_flags: u8,
    is_remote: bool,
    trace_state: String,
}

impl From<&TraceContext> for SerializedTraceContext {
    fn from(trace_context: &TraceContext) -> Self {
        let span_context = &trace_context.span_context;
        Self {
            has_span: trace_context.span.is_some(),
            trace_id: span_context.trace_id().to_string(),
            span_id: span_context.span_id().to_string(),
            trace_flags: span_context.trace_flags().to_u8(),
            is_remote: span_context.is_remote(),
            trace_state: span_context.trace_state().header(),
        }
    }
}

impl TryFrom<SerializedTraceContext> for TraceContext {
    type Error = String;

    fn try_from(serialized: SerializedTraceContext) -> Result<Self, Self::Error> {
        let span_context = SpanContext::new(
            TraceId::from_hex(&serialized.trace_id).map_err(|e| format!("invalid trace id: {e}"))?,
            SpanId::from_hex(&serialized.span_id).map_err(|e| format!("invalid span id: {e}"))?,
            TraceFlags::new(serialized.trace_flags),
            serialized.is_remote,
            TraceState::from_str(&serialized.trace_state).map_err(|e| format!("invalid trace state: {e}"))?,
        );
        Ok(Self { span: serialized.has_span.then(tracing::Span::none), span_context })
    }
}

impl Serialize for TraceContext {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SerializedTraceContext::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TraceContext {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let serialized = SerializedTraceContext::deserialize(deserializer)?;
        TraceContext::try_from(serialized).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_round_trip_preserves_the_span_context() {
        let span_context = SpanContext::new(
            TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap(),
            SpanId::from_hex("b7ad6b7169203331").unwrap(),
            TraceFlags::SAMPLED,
            true,
            TraceState::from_key_value(vec![("foo", "bar")]).unwrap(),
        );
        let trace_context = TraceContext { span: None, span_context };

        let serialized = serde_json::to_string(&trace_context).unwrap();
        let deserialized: TraceContext = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, trace_context);
        assert!(deserialized.span.is_none());
    }

    #[test]
    fn serialization_round_trip_preserves_the_presence_of_a_span() {
        let trace_context =
            TraceContext { span: Some(tracing::Span::none()), span_context: SpanContext::empty_context() };

        let serialized = serde_json::to_string(&trace_context).unwrap();
        let deserialized: TraceContext = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, trace_context);
        assert!(deserialized.span.is_some());
    }

    #[test]
    fn serialization_round_trip_preserves_the_empty_context() {
        let trace_context = TraceContext::none();

        let serialized = serde_json::to_string(&trace_context).unwrap();
        let deserialized: TraceContext = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, trace_context);
        assert!(!deserialized.span_context.is_valid());
    }
}
