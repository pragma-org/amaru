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

use crate::{consensus::store::ChainStore, ConsensusError};
use amaru_kernel::{Header, Point};
use amaru_ouroboros_traits::IsHeader;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn store_header(
    store: Arc<Mutex<dyn ChainStore<Header>>>,
    point: &Point,
    header: &Header,
) -> Result<(), ConsensusError> {
    store
        .lock()
        .await
        .store_header(&header.hash(), header)
        .map_err(|e| ConsensusError::StoreHeaderFailed(point.clone(), e))
}
