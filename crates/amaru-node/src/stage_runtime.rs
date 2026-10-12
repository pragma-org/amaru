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

use std::{sync::Arc, time::Duration};

use amaru_consensus::{
    block_validator::{BlockValidator, LedgerThreadJoinError, LedgerThreadStop},
    effects::{
        ConsensusMode, ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourcePoolSummaries,
    },
    stages::{
        adopt_chain::{self, AdoptChain},
        block_source,
        forge_block::ForgeBlockMsg,
        select_chain::SelectChainMsg,
        validate_block::{self, ValidateBlock, ValidateBlockMsg},
    },
};
use amaru_kernel::{ConsensusParameters, Epoch, EraHistory, Point};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::{ChainStore, MempoolMsg, PoolSummaries};
use amaru_protocols::{manager::ManagerMessage, store_effects::ResourceHeaderStore};
use amaru_pure_stage::{SendData, Sender, StageGraph, StageRef, tokio::TokioRunning};

use crate::{ComponentFailure, ShutdownError, ShutdownReport};

/// Install the ledger resources used by validation and adoption for every block source.
pub(crate) fn register_ledger_resources(
    stage_graph: &mut impl StageGraph,
    chain_store: Arc<dyn ChainStore>,
    pool_summaries: PoolSummaries,
    block_validator: Arc<BlockValidator>,
    consensus_parameters: Arc<ConsensusParameters>,
    era_history: EraHistory,
) {
    stage_graph.resources().put::<ResourceHeaderStore>(chain_store);
    stage_graph.resources().put::<ResourceBlockValidation>(block_validator.clone());
    stage_graph.resources().put::<ResourcePoolSummaries>(Arc::new(pool_summaries));
    stage_graph.resources().put::<ResourceConsensusParameters>(consensus_parameters);
    stage_graph.resources().put::<ResourceEraHistory>(era_history);
    stage_graph.resources().put(ConsensusMode::Sync);
}

/// Keep the stage resources current when the ledger computes a new stake distribution.
pub(crate) fn on_stake_dist_updated<M: SendData>(
    stage_graph: &impl StageGraph,
    block_validator: &BlockValidator,
    sender: Sender<M>,
    message: impl Fn(Epoch) -> M + Send + Sync + 'static,
) {
    // The validator lives in these resources; a strong capture would keep the stores open.
    let resources = stage_graph.resources().downgrade();
    block_validator.set_on_stake_dist_updated(Arc::new(move |summaries| {
        let max_epoch = summaries.max_epoch();
        resources.put::<ResourcePoolSummaries>(Arc::new(summaries));
        let send = async {
            if sender.send(message(max_epoch)).await.is_err() {
                amaru_observability::warn!(node::build::STAKE_DIST_NOTIFY_FAILED);
            }
        };
        // This runs on the ledger thread; its join also covers notification delivery.
        #[expect(clippy::expect_used)]
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("cannot build current thread runtime");
        rt.block_on(send);
    }));
}

/// Wire the ledger validation and chain adoption path for any block source.
#[expect(clippy::too_many_arguments)]
pub(crate) fn wire_validation_stages(
    stage_graph: &mut impl StageGraph,
    ledger_tip: Point,
    security_param: u64,
    manager: StageRef<ManagerMessage>,
    block_source: Option<StageRef<block_source::BlockSourceMsg>>,
    mempool: Option<StageRef<MempoolMsg>>,
    validation_results: StageRef<SelectChainMsg>,
    forge: Option<StageRef<ForgeBlockMsg>>,
) -> StageRef<ValidateBlockMsg> {
    let validate_block = stage_graph.stage("validate_block", validate_block::stage);
    let adopt_chain = stage_graph.stage("adopt_chain", adopt_chain::stage);
    let mut adopt = AdoptChain::new(manager, block_source.clone(), mempool, security_param, ledger_tip);
    if let Some(forge) = forge {
        adopt = adopt.with_forge(forge);
    }
    let adopt_chain = stage_graph.wire_up(adopt_chain, adopt);
    stage_graph
        .wire_up(
            validate_block,
            ValidateBlock::new(
                adopt_chain.without_state(),
                validation_results,
                block_source,
                security_param,
                ledger_tip,
            ),
        )
        .without_state()
}

/// Stop stages before joining their workers and releasing the stores.
pub(crate) async fn shutdown_stages(
    tokio_running: TokioRunning,
    ledger_thread: LedgerThreadStop,
    connections: Option<Arc<TokioConnections>>,
    performance: Box<dyn FnOnce() -> std::thread::Result<()> + Send + Sync>,
    timeout: Duration,
) -> Result<ShutdownReport, ShutdownError> {
    tokio_running.request_abort();
    let mut unexpected_exits = match tokio_running.join().await {
        Ok(report) => {
            report.unexpected_exits.into_iter().map(|stage| ComponentFailure::StageExited { stage }).collect()
        }
        Err(error) => vec![ComponentFailure::Stage(error.to_string())],
    };
    if let Some(connections) = connections {
        unexpected_exits.extend(
            connections.shutdown().await.into_iter().map(|error| ComponentFailure::NetworkListener(error.to_string())),
        );
    }
    let (ledger, performance) =
        tokio::task::spawn_blocking(move || (ledger_thread.join_timeout(timeout), performance()))
            .await
            .map_err(|error| ShutdownError::JoinTask { reason: error.to_string() })?;
    let performance = performance.err().map(|_| ComponentFailure::Performance("worker panicked".into()));
    let ledger = match ledger {
        Ok(()) => None,
        Err(LedgerThreadJoinError::Panicked(message)) => Some(ComponentFailure::Ledger(message)),
        Err(LedgerThreadJoinError::Timeout { timeout }) => return Err(ShutdownError::LedgerTimeout { timeout }),
    };

    unexpected_exits.extend([ledger, performance].into_iter().flatten());
    Ok(ShutdownReport { unexpected_exits })
}
