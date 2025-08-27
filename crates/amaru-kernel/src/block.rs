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

use crate::{Point, RawBlock};
use tracing::Span;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ValidateBlockEvent {
    Validated {
        point: Point,
        block: RawBlock,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl PartialEq for ValidateBlockEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Validated {
                    point: l_point,
                    block: l_block,
                    ..
                },
                Self::Validated {
                    point: r_point,
                    block: r_block,
                    ..
                },
            ) => l_point == r_point && l_block == r_block,
            (
                Self::Rollback {
                    rollback_point: l_rollback_point,
                    ..
                },
                Self::Rollback {
                    rollback_point: r_rollback_point,
                    ..
                },
            ) => l_rollback_point == r_rollback_point,
            _ => false,
        }
    }
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
