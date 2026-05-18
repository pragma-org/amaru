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

use std::fmt;

use opentelemetry::{
    Context,
    trace::{SpanContext, TraceContextExt},
};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Parent context for spans that must cross stage boundaries.
#[derive(Clone)]
pub struct TraceContext(SpanContext);

impl TraceContext {
    pub fn none() -> Self {
        Self(SpanContext::empty_context())
    }

    pub fn from_span(span: &Span) -> Self {
        let span_context = span.context().span().span_context().clone();
        Self(span_context)
    }

    pub fn context(&self) -> Context {
        if self.0.is_valid() { Context::new().with_remote_span_context(self.0.clone()) } else { Context::new() }
    }
}

impl Default for TraceContext {
    fn default() -> Self {
        Self::none()
    }
}

impl fmt::Debug for TraceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TraceContext").field(&"<opaque>").finish()
    }
}

impl PartialEq for TraceContext {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
