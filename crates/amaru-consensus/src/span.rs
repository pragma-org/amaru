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

use crate::consensus::{ChainSyncEvent, DecodedChainSyncEvent, ValidateHeaderEvent};

#[cfg(feature = "telemetry")]
mod impls {
    use super::*;
    use amaru_kernel::span::HasSpan;
    use opentelemetry::Context;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

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
}

#[cfg(not(feature = "telemetry"))]
mod impls {
    use super::*;

    impl HasSpan for ChainSyncEvent {}

    impl HasSpan for DecodedChainSyncEvent {}

    impl HasSpan for ValidateHeaderEvent {}
}
