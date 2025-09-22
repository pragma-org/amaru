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

use crate::CanValidateBlocks;
use crate::can_validate_blocks::BlockValidationError;
use amaru_kernel::{Point, RawBlock};
use amaru_metrics::MetricsPort;

/// A fake block validator that always returns the same height.
#[derive(Clone, Debug, Default)]
pub struct MockCanValidateBlocks;

#[async_trait::async_trait]
impl CanValidateBlocks for MockCanValidateBlocks {
    async fn roll_forward_block(
        &self,
        _point: &Point,
        _block: &RawBlock,
        _metrics_port: &mut MetricsPort,
    ) -> Result<Result<u64, BlockValidationError>, BlockValidationError> {
        Ok(Ok(1))
    }

    fn rollback_block(&self, _to: &Point) -> Result<(), BlockValidationError> {
        Ok(())
    }
}
