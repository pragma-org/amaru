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

use crate::peer::Peer;
use crate::{Point, RawBlock};
use pallas_primitives::babbage::Header;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
use tracing::Span;

#[derive(Clone, Serialize, Deserialize)]
pub enum ValidateBlockEvent {
    Validated {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "default_block")]
        block: RawBlock,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

fn default_block() -> RawBlock {
    RawBlock::from(Vec::new().as_slice())
}

impl Debug for ValidateBlockEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidateBlockEvent::Validated {
                peer,
                header,
                block,
                ..
            } => f
                .debug_struct("Validated")
                .field("peer", peer)
                .field("header", header)
                .field("block", block)
                .finish(),
            ValidateBlockEvent::Rollback {
                peer,
                rollback_point,
                ..
            } => f
                .debug_struct("Rollback")
                .field("peer", peer)
                .field("rollback_point", rollback_point)
                .finish(),
        }
    }
}

impl PartialEq for ValidateBlockEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ValidateBlockEvent::Validated {
                    peer: p1,
                    header: h1,
                    block: b1,
                    ..
                },
                ValidateBlockEvent::Validated {
                    peer: p2,
                    header: h2,
                    block: b2,
                    ..
                },
            ) => p1 == p2 && h1 == h2 && b1 == b2,
            (
                ValidateBlockEvent::Rollback {
                    peer: p1,
                    rollback_point: rp1,
                    ..
                },
                ValidateBlockEvent::Rollback {
                    peer: p2,
                    rollback_point: rp2,
                    ..
                },
            ) => p1 == p2 && rp1 == rp2,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockValidationResult {
    BlockValidated {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "default_block")]
        block: RawBlock,
        #[serde(skip, default = "Span::none")]
        span: Span,
        block_height: u64,
    },
    BlockValidationFailed {
        peer: Peer,
        point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    RolledBackTo {
        peer: Peer,
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl PartialEq for BlockValidationResult {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                BlockValidationResult::BlockValidated {
                    peer: p1,
                    header: hd1,
                    block: b1,
                    block_height: bh1,
                    ..
                },
                BlockValidationResult::BlockValidated {
                    peer: p2,
                    header: hd2,
                    block: b2,
                    block_height: bh2,
                    ..
                },
            ) => p1 == p2 && hd1 == hd2 && b1 == b2 && bh1 == bh2,
            (
                BlockValidationResult::BlockValidationFailed {
                    peer: p1,
                    point: pt1,
                    ..
                },
                BlockValidationResult::BlockValidationFailed {
                    peer: p2,
                    point: pt2,
                    ..
                },
            ) => p1 == p2 && pt1 == pt2,
            (
                BlockValidationResult::RolledBackTo {
                    peer: p1,
                    rollback_point: rp1,
                    ..
                },
                BlockValidationResult::RolledBackTo {
                    peer: p2,
                    rollback_point: rp2,
                    ..
                },
            ) => p1 == p2 && rp1 == rp2,
            _ => false,
        }
    }
}

#[derive(Debug)]
pub struct StageError(anyhow::Error);

impl StageError {
    pub fn new(err: anyhow::Error) -> Self {
        StageError(err)
    }
}

impl From<anyhow::Error> for StageError {
    fn from(err: anyhow::Error) -> Self {
        StageError::new(err)
    }
}

impl Display for StageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "StageError: {}", self.0)
    }
}

impl Serialize for StageError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// This deserialization implementation is a best-effort attempt to
/// recover the error message. The original error type is lost during
/// serialization, so we can only reconstruct the error message as a string.
impl<'de> Deserialize<'de> for StageError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(StageError::new(anyhow::anyhow!(s)))
    }
}

impl PartialEq for StageError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}

#[cfg(feature = "telemetry")]
mod impls {
    use super::*;
    use crate::span::HasSpan;
    use opentelemetry::Context;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

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
    use super::*;
    use crate::span::HasSpan;

    impl HasSpan for ValidateBlockEvent {}

    impl HasSpan for BlockValidationResult {}
}
