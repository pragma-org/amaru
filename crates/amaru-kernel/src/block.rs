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
use tracing::Span;

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum ValidateBlockEvent {
    Validated {
        peer: Peer,
        header: Header,
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

#[derive(Debug, Clone)]
pub enum BlockValidationResult {
    BlockValidated {
        point: Point,
        block: RawBlock,
        span: Span,
        block_height: u64,
    },
    BlockValidationFailed {
        point: Point,
        span: Span,
    },
    RolledBackTo {
        rollback_point: Point,
        span: Span,
    },
}
