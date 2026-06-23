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

use std::fmt::Display;

use opentelemetry::{
    Context, ContextGuard,
    trace::{SpanContext, TraceContextExt},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Parent context for spans that must cross stage boundaries.
/// It encapsulates an OpenTelemetry `SpanContext` to provide a few helper functions.
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
    pub fn record(&self, name: &str, value: impl Display) {
        if let Some(span) = &self.span {
            span.record(name, value.to_string());
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
