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

use amaru_kernel::consensus_events::{
    BlockValidationResult, ChainSyncEvent, DecodedChainSyncEvent, Tracked, ValidateBlockEvent,
    ValidateHeaderEvent,
};
use tracing::Span;

/// Helper trait to remove span reparenting boilerplate.
pub trait HasSpan {
    fn span(&self) -> &Span;
}

impl HasSpan for ChainSyncEvent {
    fn span(&self) -> &Span {
        match self {
            ChainSyncEvent::RollForward { span, .. } => span,
            ChainSyncEvent::Rollback { span, .. } => span,
        }
    }
}

impl HasSpan for DecodedChainSyncEvent {
    fn span(&self) -> &Span {
        match self {
            DecodedChainSyncEvent::RollForward { span, .. } => span,
            DecodedChainSyncEvent::Rollback { span, .. } => span,
        }
    }
}

impl HasSpan for ValidateHeaderEvent {
    fn span(&self) -> &Span {
        match self {
            ValidateHeaderEvent::Validated { span, .. } => span,
            ValidateHeaderEvent::Rollback { span, .. } => span,
        }
    }
}

impl HasSpan for ValidateBlockEvent {
    fn span(&self) -> &Span {
        match self {
            ValidateBlockEvent::Validated { span, .. } => span,
            ValidateBlockEvent::Rollback { span, .. } => span,
        }
    }
}

impl HasSpan for BlockValidationResult {
    fn span(&self) -> &Span {
        match self {
            BlockValidationResult::BlockValidated { span, .. } => span,
            BlockValidationResult::BlockValidationFailed { span, .. } => span,
            BlockValidationResult::RolledBackTo { span, .. } => span,
        }
    }
}

impl<T: HasSpan> HasSpan for Tracked<T> {
    fn span(&self) -> &Span {
        match self {
            Tracked::Wrapped(e) => e.span(),
            Tracked::CaughtUp { span, .. } => span,
        }
    }
}
