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

use std::sync::Arc;

use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::errors::{ConsensusError, ProcessingFailed};
use crate::consensus::span::HasSpan;
use amaru_kernel::consensus_events::BlockValidationResult;
use amaru_kernel::{BlockHeader, IsHeader};
use amaru_ouroboros::{ChainStore, StoreError};
use pure_stage::StageRef;
use tracing::{Level, span};

type State = (
    // downstream stage to which we simply pass along events
    StageRef<BlockValidationResult>,
    // where to send processing errors
    StageRef<ProcessingFailed>,
);

/// The `store_chain` stage is responsible for persisting the state of the _best chain_ in the `ChainStore`
///
/// * When the chain is extended, it simply records the new block's point as an extension of the best chain
/// * When the chain is rolled back, it discards the tip of the chain until before the rollback point
///
/// The reason this is a stage and not an `Effect` is because it's
/// critically important we maintain a consistent best chain in the
/// store and do not forward any invalid information should there be a
/// failure or we detect an inconsistency.
pub async fn stage(state: State, msg: BlockValidationResult, eff: impl ConsensusOps) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.store_chain");
    let _entered = span.enter();

    let (downstream, processing_errors) = state;
    match msg {
        BlockValidationResult::BlockValidated { .. } => eff.base().send(&downstream, msg).await,
        BlockValidationResult::RolledBackTo { .. } => eff.base().send(&downstream, msg).await,
        BlockValidationResult::BlockValidationFailed { .. } => {
            eff.base().send(&downstream, msg).await
        }
    }
    (downstream, processing_errors)
}

/// State of chain store
pub struct StoreChain {
    /// Current tip
    tip: BlockHeader,
}

impl StoreChain {
    fn new(tip: &BlockHeader) -> Self {
        Self { tip: tip.clone() }
    }

    pub async fn forward_block(
        &self,
        store: Arc<dyn ChainStore<BlockHeader>>,
        header: &BlockHeader,
    ) -> Result<(), ConsensusError> {
        store
            .set_best_chain(header)
            .map_err(|e| ConsensusError::SetBestChainHashFailed(header.hash(), e))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{BlockHeader, IsHeader, peer::Peer};
    use amaru_ouroboros::ChainStore;
    use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

    use crate::consensus::{
        headers_tree::data_generation::generate_headers_chain, stages::store_chain::StoreChain,
    };

    #[tokio::test]
    async fn update_best_chain_to_block_slot_given_new_block_is_valid() {
        let chain = generate_headers_chain(10);
        let store = create_db();
        let store_chain = StoreChain::new(&chain[0]);

        for i in 1..10 {
            let header = &chain[i];
            store_chain.forward_block(store.clone(), header).await;
        }

        assert_eq!(
            store.load_from_best_chain(&chain[9].point()),
            Some(chain[9].hash())
        );
    }

    fn create_db() -> Arc<dyn ChainStore<BlockHeader>> {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = path.join("rocksdb_chain_store");
        use std::fs::create_dir_all;
        create_dir_all(&basedir).expect("fail to create test dir");
        let config = RocksDbConfig::new(basedir);

        let store = RocksDBStore::create(config).expect("fail to initialise RocksDB");
        Arc::new(store)
    }
}
