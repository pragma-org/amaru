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
use crate::consensus::tip::{AsHeaderTip, HeaderTip};
use amaru_kernel::consensus_events::BlockValidationResult;
use amaru_kernel::{BlockHeader, IsHeader, Point};
use amaru_ouroboros::{ChainStore, StoreError};
use pure_stage::StageRef;
use serde::{Deserialize, Serialize};
use tracing::{Level, span};

type State = (
    // current state
    StoreChain,
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
    let store = eff.store();

    let (mut store_chain, downstream, processing_errors) = state;
    match msg {
        BlockValidationResult::BlockValidated { ref header, .. } => {
            match store_chain.roll_forward(store, header).await {
                Ok(()) => eff.base().send(&downstream, msg).await,
                Err(e) => {
                    eff.base()
                        .send(
                            &processing_errors,
                            ProcessingFailed {
                                peer: None,
                                error: e.into(),
                            },
                        )
                        .await
                }
            }
        }
        BlockValidationResult::RolledBackTo {
            ref rollback_header,
            ..
        } => match store_chain.rollback(store, &rollback_header.point()).await {
            Ok(_size) => eff.base().send(&downstream, msg).await,
            Err(e) => {
                eff.base()
                    .send(
                        &processing_errors,
                        ProcessingFailed {
                            peer: None,
                            error: e.into(),
                        },
                    )
                    .await
            }
        },

        BlockValidationResult::BlockValidationFailed { .. } => {
            eff.base().send(&downstream, msg).await
        }
    }
    (store_chain, downstream, processing_errors)
}

/// State of chain store
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Default)]
pub struct StoreChain {
    /// Current tip
    tip: HeaderTip,
}

impl StoreChain {
    pub fn new(tip: &HeaderTip) -> Self {
        Self { tip: tip.clone() }
    }

    pub async fn roll_forward(
        &mut self,
        store: Arc<dyn ChainStore<BlockHeader>>,
        header: &BlockHeader,
    ) -> Result<(), ConsensusError> {
        let result = store
            .roll_forward_chain(&header.point())
            .map_err(|e| ConsensusError::SetBestChainHashFailed(header.hash(), e));
        self.tip = header.as_header_tip();
        result
    }

    pub async fn rollback(
        &mut self,
        store: Arc<dyn ChainStore<BlockHeader>>,
        point: &Point,
    ) -> Result<usize, ConsensusError> {
        store
            .roll_back_chain(point)
            .map_err(|e| ConsensusError::SetBestChainHashFailed(point.hash(), e))
            .map(|sz| {
                self.tip = HeaderTip::new(point.clone(), self.tip.block_height() - sz as u64);
                sz
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{assert_matches::assert_matches, sync::Arc};

    use amaru_kernel::{
        BlockHeader, IsHeader, consensus_events::BlockValidationResult, peer::Peer,
    };
    use amaru_ouroboros::ChainStore;
    use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

    use crate::consensus::{
        errors::ConsensusError,
        headers_tree::data_generation::{generate_headers_chain, generate_headers_chain_from},
        stages::store_chain::StoreChain,
        tip::AsHeaderTip,
    };

    #[tokio::test]
    async fn update_best_chain_to_block_slot_given_new_block_is_valid() -> anyhow::Result<()> {
        let (store, chain, mut store_chain) = populate_db().await?;
        let new_tip = generate_headers_chain_from(1, &chain[9])[0].clone();

        store_chain.roll_forward(store.clone(), &new_tip).await?;

        assert_eq!(
            store.load_from_best_chain(&new_tip.point()),
            Some(new_tip.hash())
        );
        assert_eq!(store_chain.tip, new_tip.as_header_tip());
        Ok(())
    }

    #[tokio::test]
    async fn update_best_chain_to_rollback_point() -> anyhow::Result<()> {
        let (store, chain, mut store_chain) = populate_db().await?;

        store_chain
            .rollback(store.clone(), &chain[5].point())
            .await?;

        assert_eq!(store.load_from_best_chain(&chain[9].point()), None);
        assert_eq!(
            store.load_from_best_chain(&chain[5].point()),
            Some(chain[5].hash())
        );
        assert_eq!(store_chain.tip, chain[5].as_header_tip());
        Ok(())
    }

    #[tokio::test]
    async fn raises_error_if_forward_is_not_a_child_of_best_chain() -> anyhow::Result<()> {
        let (store, chain, mut store_chain) = populate_db().await?;

        let result = store_chain.roll_forward(store.clone(), &chain[8]).await;

        match result {
            Ok(()) => panic!("expected test to fail"),
            Err(e) => {
                assert_matches!(e, ConsensusError::SetBestChainHashFailed(_, _));
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn raises_error_if_rollback_is_not_on_best_chain() -> anyhow::Result<()> {
        let (store, chain, mut store_chain) = populate_db().await?;
        let new_tip = generate_headers_chain_from(1, &chain[6])[0].clone();

        let result = store_chain.rollback(store.clone(), &new_tip.point()).await;

        match result {
            Ok(_) => panic!("expected test to fail"),
            Err(e) => {
                assert_matches!(e, ConsensusError::SetBestChainHashFailed(_, _));
                Ok(())
            }
        }
    }

    // HELPERS

    async fn populate_db() -> anyhow::Result<(
        Arc<dyn ChainStore<BlockHeader>>,
        Vec<BlockHeader>,
        StoreChain,
    )> {
        let chain = generate_headers_chain(10);
        let store = create_db();
        let mut store_chain = StoreChain::new(&chain[0].as_header_tip());

        for i in 1..10 {
            let header = &chain[i];
            store.set_best_chain_hash(&header.hash())?;
            store_chain.roll_forward(store.clone(), header).await?;
            store.store_header(&header)?;
        }
        Ok((store, chain, store_chain))
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
