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

use crate::can_fetch_block::{BlockFetchClientError, CanFetchBlock};
use amaru_kernel::Point;
use async_trait::async_trait;

/// A block fetcher.
#[derive(Clone, Debug, Default)]
pub struct MockCanFetchBlock;

#[async_trait]
impl CanFetchBlock for MockCanFetchBlock {
    async fn fetch_block(&self, _point: &Point) -> Result<Vec<u8>, BlockFetchClientError> {
        Ok(vec![])
    }
}
