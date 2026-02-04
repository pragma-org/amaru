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

use crate::{
    CanValidateBlocks,
    can_validate_blocks::{BlockValidationError, CanValidateHeaders, HeaderValidationError},
};
use amaru_kernel::{Block, BlockHeader, Point};
use amaru_metrics::ledger::LedgerMetrics;

/// A fake block validator that always returns the same height.
#[derive(Clone, Debug, Default)]
pub struct MockCanValidateBlocks;

#[async_trait::async_trait]
impl CanValidateBlocks for MockCanValidateBlocks {
    async fn roll_forward_block(
        &self,
        _point: &Point,
        _block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        Ok(Ok(Default::default()))
    }

    fn rollback_block(&self, _to: &Point) -> Result<(), BlockValidationError> {
        Ok(())
    }
}

/// A fake header validator that always returns ok
#[derive(Clone, Debug, Default)]
pub struct MockCanValidateHeaders;

impl CanValidateHeaders for MockCanValidateHeaders {
    fn validate_header(&self, _header: &BlockHeader) -> Result<(), HeaderValidationError> {
        Ok(())
    }
}
