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

pub use impls::*;

#[cfg(feature = "telemetry")]
mod impls {
    use crate::consensus::events::{
        BlockValidationResult, ChainSyncEvent, DecodedChainSyncEvent, ValidateBlockEvent,
        ValidateHeaderEvent,
    };
    use opentelemetry::Context;
    use tracing::Span;
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

    impl HasSpan for ChainSyncEvent {
        fn context(&self) -> Context {
            match self {
                ChainSyncEvent::RollForward { span, .. } => span.context(),
                ChainSyncEvent::Rollback { span, .. } => span.context(),
                ChainSyncEvent::CaughtUp { span, .. } => span.context(),
            }
        }
    }

    impl HasSpan for DecodedChainSyncEvent {
        fn context(&self) -> Context {
            match self {
                DecodedChainSyncEvent::RollForward { span, .. } => span.context(),
                DecodedChainSyncEvent::Rollback { span, .. } => span.context(),
                DecodedChainSyncEvent::CaughtUp { span, .. } => span.context(),
            }
        }
    }

    impl HasSpan for ValidateHeaderEvent {
        fn context(&self) -> Context {
            match self {
                ValidateHeaderEvent::Validated { span, .. } => span.context(),
                ValidateHeaderEvent::Rollback { span, .. } => span.context(),
            }
        }
    }

    impl HasSpan for ValidateBlockEvent {
        fn context(&self) -> Context {
            match self {
                ValidateBlockEvent::Validated { span, .. } => span.context(),
                ValidateBlockEvent::Rollback { span, .. } => span.context(),
            }
        }
    }

    impl HasSpan for BlockValidationResult {
        fn context(&self) -> Context {
            match self {
                BlockValidationResult::BlockValidated { span, .. } => span.context(),
                BlockValidationResult::BlockValidationFailed { span, .. } => span.context(),
                BlockValidationResult::RolledBackTo { span, .. } => span.context(),
            }
        }
    }
}

#[cfg(not(feature = "telemetry"))]
mod impls {
    use crate::consensus::events::*;
    use tracing::Span;

    pub fn adopt_current_span(_has_span: &impl HasSpan) -> Span {
        Span::current()
    }

    pub trait HasSpan {}

    impl HasSpan for ChainSyncEvent {}

    impl HasSpan for DecodedChainSyncEvent {}

    impl HasSpan for ValidateHeaderEvent {}

    impl HasSpan for ValidateBlockEvent {}

    impl HasSpan for BlockValidationResult {}
}
