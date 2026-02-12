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

use crate::simulator::Args;
use amaru::tests::configuration::NodeTestConfig;
use amaru::tests::setup::create_node;
use amaru_kernel::cardano::network_block::NETWORK_BLOCK;
use amaru_kernel::{BlockHeader, HeaderHash, Point, RawBlock};
use amaru_ouroboros::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use amaru_protocols::store_effects::ResourceHeaderStore;
use delegate::delegate;
use pure_stage::StageGraph;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::trace_buffer::TraceEntry;
use std::sync::Arc;

/// Replay a previous simulation run:
pub fn replay(args: Args, traces: Vec<TraceEntry>) -> anyhow::Result<()> {
    let node_config = NodeTestConfig::default()
        .with_chain_length(args.generated_chain_depth)
        .with_seed(
            args.seed
                .expect("there must be a seed to replay a simulation"),
        );

    let mut stage_graph = SimulationBuilder::default();
    let _ = create_node(&node_config, &mut stage_graph)?;
    stage_graph
        .resources()
        .put::<ResourceHeaderStore>(Arc::new(ReplayStore::default()));
    let mut replay = stage_graph.replay();
    replay.run_trace(traces)
}

#[derive(Clone, Default)]
struct ReplayStore {
    inner: InMemConsensusStore<BlockHeader>,
}

impl ReadOnlyChainStore<BlockHeader> for ReplayStore {
    delegate! {
        to self.inner {
            fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader>;
            fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash>;
            fn get_anchor_hash(&self) -> HeaderHash;
            fn get_best_chain_hash(&self) -> HeaderHash;
            fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash>;
            fn next_best_chain(&self, point: &Point) -> Option<Point>;
            fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces>;
            fn has_header(&self, hash: &HeaderHash) -> bool;
        }
    }

    fn load_block(&self, _hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        Ok(Some(NETWORK_BLOCK.raw_block()))
    }
}

impl ChainStore<BlockHeader> for ReplayStore {
    delegate! {
        to self.inner {
            fn store_header(&self, header: &BlockHeader) -> Result<(), StoreError>;
            fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;
            fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;
            fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError>;
            fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError>;
            fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError>;
            fn rollback_chain(&self, point: &Point) -> Result<usize, StoreError>;
        }
    }
}
