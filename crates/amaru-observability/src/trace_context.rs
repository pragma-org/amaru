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

use std::{fmt, marker::PhantomData, ops::Deref, str::FromStr};

use opentelemetry::{
    Context, ContextGuard,
    trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Marker for [`TraceContext::none`]: not a parent of any span.
pub struct NoParent;

/// Shared parent of chain selection after either chain-sync process span.
///
/// Rolling a peer forward and rolling it backward are the same header arrival,
/// so chain selection opens under whichever of those process spans ran. A
/// mempool span or a connection span is not that arrival and does not convert
/// into this marker.
pub struct ChainSyncProcess;

/// Parent carried with a block once chain selection has a context to forward.
///
/// Fetch, validation, adoption, header forwarding, and a ban opened from that
/// work share one message field. The field is filled from [`ChainSyncProcess`],
/// from a chain-selection span, or from startup recovery. The span bytes stay
/// as they were; only this marker changes.
pub struct ContinuedHeader;

/// Context attached by an effect that opens no child span.
///
/// Not a parent: no span lists it. The caller's marker stays on the stage
/// value that built the effect.
pub struct AttachedSpan;

/// Borrowed or owned [`TraceContext`], so `parent_context:` accepts either.
#[doc(hidden)]
pub trait ParentContext {
    type Schema;

    fn as_trace_context(&self) -> &TraceContext<Self::Schema>;
}

impl<S> ParentContext for TraceContext<S> {
    type Schema = S;

    fn as_trace_context(&self) -> &TraceContext<S> {
        self
    }
}

impl<S> ParentContext for &TraceContext<S> {
    type Schema = S;

    fn as_trace_context(&self) -> &TraceContext<S> {
        self
    }
}

/// A child span implements this for each marker named in its `parents:` list.
#[diagnostic::on_unimplemented(
    message = "this span context is not a parent of `{Self}`",
    label = "`{Self}` does not accept this `TraceContext`",
    note = "name the parent in the child span's `parents:` list"
)]
pub trait AcceptsParent<P> {}

/// OpenTelemetry span context tagged with the schema `S` it was taken from.
///
/// `S` is a phantom marker. It is not part of the serialized form, so two
/// contexts that name different schemas encode as the same bytes.
pub struct TraceContext<S> {
    span_context: SpanContext,
    _schema: PhantomData<fn() -> S>,
}

impl<S> Clone for TraceContext<S> {
    fn clone(&self) -> Self {
        Self { span_context: self.span_context.clone(), _schema: PhantomData }
    }
}

impl<S> fmt::Debug for TraceContext<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TraceContext").field("span_context", &self.span_context).finish()
    }
}

impl<S> PartialEq for TraceContext<S> {
    fn eq(&self, other: &Self) -> bool {
        self.span_context == other.span_context
    }
}

impl<S> Eq for TraceContext<S> {}

impl TraceContext<NoParent> {
    /// Empty context. Not a parent of a span that lists parents.
    pub fn none() -> Self {
        Self::from_span_context(SpanContext::empty_context())
    }
}

impl Default for TraceContext<NoParent> {
    fn default() -> Self {
        Self::none()
    }
}

impl<S> TraceContext<S> {
    pub fn from_span_context(span_context: SpanContext) -> Self {
        Self { span_context, _schema: PhantomData }
    }

    /// Invalid span context, typed as `S`.
    ///
    /// Same bytes as [`TraceContext::<NoParent>::none`]. Use it only where a
    /// message field has type `S` and no span was captured. It does not make
    /// [`NoParent`] a legal parent.
    pub fn detached() -> Self {
        Self::from_span_context(SpanContext::empty_context())
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

    /// Bytes for an effect that only attaches this context.
    pub fn for_attach(&self) -> TraceContext<AttachedSpan> {
        TraceContext { span_context: self.span_context.clone(), _schema: PhantomData }
    }
}

impl From<TraceContext<crate::amaru::consensus::roll_forward::PROCESS>> for TraceContext<ChainSyncProcess> {
    fn from(context: TraceContext<crate::amaru::consensus::roll_forward::PROCESS>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

impl From<TraceContext<crate::amaru::consensus::roll_backward::PROCESS>> for TraceContext<ChainSyncProcess> {
    fn from(context: TraceContext<crate::amaru::consensus::roll_backward::PROCESS>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

impl From<TraceContext<ChainSyncProcess>> for TraceContext<ContinuedHeader> {
    fn from(context: TraceContext<ChainSyncProcess>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

impl From<TraceContext<crate::amaru::consensus::chain::SELECT_FROM_BLOCK_VALIDATION>>
    for TraceContext<ContinuedHeader>
{
    fn from(context: TraceContext<crate::amaru::consensus::chain::SELECT_FROM_BLOCK_VALIDATION>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

impl From<TraceContext<crate::amaru::consensus::chain::FETCH_NEXT>> for TraceContext<ContinuedHeader> {
    fn from(context: TraceContext<crate::amaru::consensus::chain::FETCH_NEXT>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

impl From<TraceContext<crate::amaru::consensus::blocks::RECOVER_STORED>> for TraceContext<ContinuedHeader> {
    fn from(context: TraceContext<crate::amaru::consensus::blocks::RECOVER_STORED>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

impl From<TraceContext<crate::amaru::consensus::node::INITIALIZE>> for TraceContext<ContinuedHeader> {
    fn from(context: TraceContext<crate::amaru::consensus::node::INITIALIZE>) -> Self {
        Self::from_span_context(context.span_context)
    }
}

/// Span opened for schema `S`.
///
/// Dereferences to [`tracing::Span`] for `&self` methods such as `enter` and
/// `in_scope`. `entered` takes the span by value, so it is implemented here.
/// [`Future::instrument`](tracing::Instrument::instrument) takes a
/// [`tracing::Span`] by value; convert with `span.into()`.
#[derive(Clone)]
pub struct SchemaSpan<S> {
    span: tracing::Span,
    _schema: PhantomData<fn() -> S>,
}

impl<S> SchemaSpan<S> {
    #[doc(hidden)]
    pub fn from_span(span: tracing::Span) -> Self {
        Self { span, _schema: PhantomData }
    }

    pub fn entered(self) -> tracing::span::EnteredSpan {
        self.span.entered()
    }
}

impl<S> Deref for SchemaSpan<S> {
    type Target = tracing::Span;

    fn deref(&self) -> &tracing::Span {
        &self.span
    }
}

impl<S> From<SchemaSpan<S>> for tracing::Span {
    fn from(span: SchemaSpan<S>) -> tracing::Span {
        span.span
    }
}

impl<S> From<&SchemaSpan<S>> for TraceContext<S> {
    fn from(span: &SchemaSpan<S>) -> Self {
        Self::from_span_context(span.span.context().span().span_context().clone())
    }
}

/// Serializable representation of a [`TraceContext`]. The schema marker is omitted.
#[derive(Serialize, Deserialize)]
struct SerializedTraceContext {
    trace_id: String,
    span_id: String,
    trace_flags: u8,
    is_remote: bool,
    trace_state: String,
}

impl<S> From<&TraceContext<S>> for SerializedTraceContext {
    fn from(trace_context: &TraceContext<S>) -> Self {
        let span_context = &trace_context.span_context;
        Self {
            trace_id: span_context.trace_id().to_string(),
            span_id: span_context.span_id().to_string(),
            trace_flags: span_context.trace_flags().to_u8(),
            is_remote: span_context.is_remote(),
            trace_state: span_context.trace_state().header(),
        }
    }
}

impl<S> TryFrom<SerializedTraceContext> for TraceContext<S> {
    type Error = String;

    fn try_from(serialized: SerializedTraceContext) -> Result<Self, Self::Error> {
        let span_context = SpanContext::new(
            TraceId::from_hex(&serialized.trace_id).map_err(|e| format!("invalid trace id: {e}"))?,
            SpanId::from_hex(&serialized.span_id).map_err(|e| format!("invalid span id: {e}"))?,
            TraceFlags::new(serialized.trace_flags),
            serialized.is_remote,
            TraceState::from_str(&serialized.trace_state).map_err(|e| format!("invalid trace state: {e}"))?,
        );
        Ok(Self::from_span_context(span_context))
    }
}

impl<S> Serialize for TraceContext<S> {
    fn serialize<Ser: Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        SerializedTraceContext::from(self).serialize(serializer)
    }
}

impl<'de, S> Deserialize<'de> for TraceContext<S> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let serialized = SerializedTraceContext::deserialize(deserializer)?;
        TraceContext::try_from(serialized).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> SpanContext {
        SpanContext::new(
            TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap(),
            SpanId::from_hex("b7ad6b7169203331").unwrap(),
            TraceFlags::SAMPLED,
            true,
            TraceState::from_key_value(vec![("foo", "bar")]).unwrap(),
        )
    }

    #[test]
    fn serialization_round_trip_preserves_the_span_context() {
        let trace_context = TraceContext::<NoParent>::from_span_context(sample_context());

        let serialized = serde_json::to_string(&trace_context).unwrap();
        let deserialized: TraceContext<NoParent> = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, trace_context);
    }

    #[test]
    fn serialization_round_trip_preserves_the_empty_context() {
        let trace_context = TraceContext::none();

        let serialized = serde_json::to_string(&trace_context).unwrap();
        let deserialized: TraceContext<NoParent> = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, trace_context);
        assert!(!deserialized.span_context.is_valid());
    }

    #[test]
    fn schema_marker_is_not_part_of_the_json() {
        struct A;
        struct B;
        let span_context = sample_context();
        let context_a = TraceContext::<A>::from_span_context(span_context.clone());
        let context_b = TraceContext::<B>::from_span_context(span_context);

        let json_a = serde_json::to_value(&context_a).unwrap();
        let json_b = serde_json::to_value(&context_b).unwrap();

        assert_eq!(json_a, json_b);
        let object = json_a.as_object().unwrap();
        assert!(object.contains_key("trace_id"));
        assert!(object.contains_key("span_id"));
        assert!(object.keys().all(|key| !key.contains("schema") && *key != "A" && *key != "B"));
    }
}
