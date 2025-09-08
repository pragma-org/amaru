// Copyright 2025 PRAGMA
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

use tracing::Span;

pub use impls::*;

#[cfg(feature = "telemetry")]
mod impls {
    use super::*;
    use opentelemetry::Context;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    /// Make current span a child of given span.
    ///
    /// This is needed to ensure tracing keeps track of dependencies between
    /// stages, properly connecting related spans even though they are crossing
    /// thread boundaries.
    pub fn adopt_current_span(has_span: &impl HasSpan) -> Span {
        let span = Span::current();
        span.set_parent(has_span.context());
        span
    }

    /// Helper trait to remove span reparenting boilerplate.
    pub trait HasSpan {
        fn context(&self) -> Context;
    }
}

#[cfg(not(feature = "telemetry"))]
mod impls {
    use super::*;

    pub fn adopt_current_span(_has_span: &impl HasSpan) -> Span {
        Span::current()
    }

    pub trait HasSpan {}
}
