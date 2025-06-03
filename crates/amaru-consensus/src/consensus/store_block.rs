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
use amaru_kernel::{block::ValidateBlockEvent, Header, Point, RawBlock};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct StoreBlock {
    store: Arc<Mutex<dyn ChainStore<Header>>>,
}

impl StoreBlock {
    pub fn new(chain_store: Arc<Mutex<dyn ChainStore<Header>>>) -> Self {
        StoreBlock { store: chain_store }
    }

    pub async fn store(&self, point: &Point, block: &RawBlock) -> Result<(), ConsensusError> {
        self.store
            .lock()
            .await
            .store_block(&point.into(), block)
            .map_err(|e| ConsensusError::StoreBlockFailed(point.clone(), e))
    }

    pub async fn handle_event(
        &self,
        event: &ValidateBlockEvent,
    ) -> Result<ValidateBlockEvent, ConsensusError> {
        match event {
            ValidateBlockEvent::Validated {
                ref point,
                ref block,
                ..
            } => {
                self.store(point, block).await?;
                Ok(event.clone())
            }
            ValidateBlockEvent::Rollback { .. } => Ok(event.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::consensus::store::StoreError;

    use super::*;
    use amaru_kernel::{Hash, Point, RawBlock};
    use std::{collections::BTreeMap, sync::Arc};
    use tokio::sync::Mutex;
    use tracing::Span;

    // Mock implementation of ChainStore for testing
    struct MockChainStore {
        stored_blocks: BTreeMap<Hash<32>, RawBlock>,
    }

    impl MockChainStore {
        fn new() -> Self {
            Self {
                stored_blocks: BTreeMap::new(),
            }
        }
    }

    impl ChainStore<Header> for MockChainStore {
        fn store_block(&mut self, point: &Hash<32>, block: &RawBlock) -> Result<(), StoreError> {
            self.stored_blocks.insert(*point, block.clone());
            Ok(())
        }

        fn load_header(&self, _hash: &Hash<32>) -> Option<Header> {
            unimplemented!()
        }

        fn store_header(&mut self, _hash: &Hash<32>, _header: &Header) -> Result<(), StoreError> {
            unimplemented!()
        }

        fn load_block(&self, _hash: &Hash<32>) -> Result<RawBlock, StoreError> {
            unimplemented!()
        }

        fn get_nonces(&self, _header: &Hash<32>) -> Option<amaru_ouroboros::Nonces> {
            unimplemented!()
        }

        fn put_nonces(
            &mut self,
            _header: &Hash<32>,
            _nonces: &amaru_ouroboros::Nonces,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }

        fn era_history(&self) -> &amaru_kernel::EraHistory {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn handle_event_returns_passed_event_when_forwarding_given_store_succeeds() {
        let mock_store = Arc::new(Mutex::new(MockChainStore::new()));
        let store_block = StoreBlock::new(mock_store.clone());

        let event = ValidateBlockEvent::Validated {
            point: Point::Specific(123, Hash::from([1; 32]).to_vec()),
            block: vec![0, 1, 2, 3],
            span: Span::current(),
        };

        let result = store_block.handle_event(&event).await;

        // we don't care about checking the data is stored properly as the underlying
        // storage is a mock anyway, we just verify that IF the storage does not
        // fail, we return ()
        assert!(result.is_ok());
    }

    #[allow(clippy::wildcard_enum_match_arm)]
    #[tokio::test]
    async fn handle_event_returns_passed_event_when_rollbacking() {
        let mock_store = Arc::new(Mutex::new(MockChainStore::new()));
        let store_block = StoreBlock::new(mock_store.clone());

        let expected_rollback_point = Point::Specific(100, Hash::from([2; 32]).to_vec());

        let event = ValidateBlockEvent::Rollback {
            rollback_point: expected_rollback_point.clone(),
            span: Span::current(),
        };

        let result = store_block.handle_event(&event).await.unwrap();

        match result {
            ValidateBlockEvent::Validated { .. } => {
                panic!("expected Rollback event")
            }
            ValidateBlockEvent::Rollback { rollback_point, .. } => {
                assert_eq!(rollback_point, expected_rollback_point)
            }
        }
    }
}
