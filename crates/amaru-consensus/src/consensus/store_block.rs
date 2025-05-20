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
        event: ValidateBlockEvent,
    ) -> Result<ValidateBlockEvent, ConsensusError> {
        match event {
            ValidateBlockEvent::Validated {
                ref point,
                ref block,
                ..
            } => {
                self.store(point, block).await?;
                Ok(event)
            }
            ValidateBlockEvent::Rollback { .. } => Ok(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::consensus::store::StoreError;

    use super::*;
    use amaru_kernel::{Hash, Point, RawBlock};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tracing::Span;

    // Mock implementation of ChainStore for testing
    struct MockChainStore {
        stored_blocks: std::collections::HashMap<Hash<32>, RawBlock>,
    }

    impl MockChainStore {
        fn new() -> Self {
            Self {
                stored_blocks: std::collections::HashMap::new(),
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

        fn load_block(&self, _hash: &Hash<32>) -> Option<RawBlock> {
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
    async fn test_handle_event_block_validated() {
        // Setup
        let mock_store = Arc::new(Mutex::new(MockChainStore::new()));
        let store_block = StoreBlock::new(mock_store.clone());

        // Create test data
        let point = Point::Specific(123, Hash::from([1; 32]).to_vec());
        let block = vec![0, 1, 2, 3];
        let span = Span::current();

        // Create a BlockValidated event
        let event = ValidateBlockEvent::Validated {
            point: point.clone(),
            block: block.clone(),
            span,
        };

        // Call handle_event
        let result = store_block.handle_event(event).await;

        // Since the implementation is unimplemented!(), this will panic
        // Once implemented, we would expect:
        assert!(result.is_ok());
    }

    #[allow(clippy::wildcard_enum_match_arm)]
    #[tokio::test]
    async fn test_handle_event_rolled_back_to() {
        // Setup
        let mock_store = Arc::new(Mutex::new(MockChainStore::new()));
        let store_block = StoreBlock::new(mock_store.clone());

        // Create test data
        let rollback_point = Point::Specific(100, Hash::from([2; 32]).to_vec());
        let span = Span::current();

        // Create a RolledBackTo event
        let event = ValidateBlockEvent::Rollback {
            rollback_point: rollback_point.clone(),
            span,
        };

        // Call handle_event
        let result = store_block.handle_event(event).await;

        // Verify the result
        assert!(result.is_ok());
        let result_event = result.unwrap();
        match result_event {
            ValidateBlockEvent::Rollback {
                rollback_point: result_point,
                span: _,
            } => {
                assert_eq!(result_point, rollback_point);
            }
            _ => panic!("Expected RolledBackTo event"),
        }
    }
}
