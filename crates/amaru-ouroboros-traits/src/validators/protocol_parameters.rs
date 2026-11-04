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

use amaru_kernel::{BlockBuilderConstraints, Point};
use async_trait::async_trait;

use crate::LedgerThreadTerminated;

/// Constraints read from the ledger together with the tip they describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBuilderConstraintsResponse {
    pub tip: Point,
    pub constraints: BlockBuilderConstraints,
}

/// Reads the block-building limits currently active in the ledger's volatile view.
#[async_trait]
pub trait CanReadBlockBuilderConstraints: Send + Sync {
    async fn block_builder_constraints(&self) -> Result<BlockBuilderConstraintsResponse, LedgerThreadTerminated>;
}
