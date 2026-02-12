// Copyright 2024 PRAGMA
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

use amaru_consensus::errors::ConsensusError::*;
use amaru_consensus::stages::accept::PullAccept;
use amaru_consensus::stages::pull::SyncTracker;
use amaru_consensus::stages::{accept, pull};
use amaru_consensus::{
    effects::ConsensusEffects,
    errors::{ProcessingFailed, ValidationFailed},
    stages::{
        fetch_block, forward_chain, receive_header,
        select_chain::{self, SelectChain},
        validate_block, validate_header,
    },
};
use amaru_kernel::Tip;
use amaru_protocols::manager;
use amaru_protocols::manager::{Manager, ManagerMessage};
use pure_stage::{Effects, SendData, StageGraph, StageRef};

/// Create a graph of processing stages for the node.
///
/// Notes:
///
/// - The `manager::stage` will dynamically create stages when getting peer connections.
/// - Two stages are collecting failures and errors. According to https://www.reactivemanifesto.org/glossary#Failure:
///     - "A failure is an unexpected event within a service that prevents it from continuing to function normally"
///     - "An error, ... is an expected and coded-for condition"
///
/// We terminate the node in case of a failure, while we just log errors.
///
#[expect(clippy::expect_used)]
pub fn build_stage_graph(
    chain_selector: SelectChain,
    sync_tracker: SyncTracker,
    manager: Manager,
    our_tip: Tip,
    stage_graph: &mut impl StageGraph,
) -> StageRef<ManagerMessage> {
    let receive_header_stage = stage_graph.stage(
        "receive_header",
        with_consensus_effects(receive_header::stage),
    );
    let validate_header_stage = stage_graph.stage(
        "validate_header",
        with_consensus_effects(validate_header::stage),
    );
    let fetch_block_stage =
        stage_graph.stage("fetch_block", with_consensus_effects(fetch_block::stage));
    let validate_block_stage = stage_graph.stage(
        "validate_block",
        with_consensus_effects(validate_block::stage),
    );
    let select_chain_stage =
        stage_graph.stage("select_chain", with_consensus_effects(select_chain::stage));
    let forward_chain_stage = stage_graph.stage(
        "forward_chain",
        with_consensus_effects(forward_chain::stage),
    );
    let accept_stage = stage_graph.stage("accept", accept::stage);

    let validation_errors_stage = stage_graph.stage(
        "validation_errors",
        async move |manager_stage, error, eff| {
            let ValidationFailed { peer, error } = error;
            match error {
                MissingTip
                | InvalidHeaderHeight { .. }
                | InvalidHeader(_, _)
                | InvalidHeaderPoint(_)
                | InvalidHeaderVariant(_)
                | HeaderPointMismatch { .. }
                | UnknownPoint(_)
                | InvalidRollback { .. }
                | InvalidBlock { .. }
                | NoncesError(_)
                | InvalidHeaderParent(_)
                | RollForwardChainFailed(_, _)
                | RollbackChainFailed(_, _)
                | FetchBlockFailed(_)
                | CannotDecodeHeader { .. } | EraHistoryError(_) => {
                    tracing::warn!(%peer, %error, "peer sent invalid data, disconnecting");
                    eff.send(&manager_stage, ManagerMessage::RemovePeer(peer)).await;
                }
                 StoreHeaderFailed(_, _)
                | RemoveHeaderFailed(_, _)
                | SetAnchorHashFailed(_, _)
                | SetBestChainHashFailed(_, _)
                | UpdateBestChainFailed(_, _, _)
                | StoreBlockFailed(_, _)
                | RollbackBlockFailed(_, _) // this can fail if the block was not downloaded in the first place
                | UnknownPeer(_) | EraNameMismatch { .. } => {
                    tracing::error!(%peer, %error, "internal error");
                }
            }
            manager_stage
        },
    );

    let processing_errors_stage = stage_graph.stage(
        "processing_errors",
        async |_, error: ProcessingFailed, eff| {
            tracing::error!(%error, "stage error");
            // termination here will tear down the entire stage graph
            eff.terminate().await
        },
    );

    let manager_stage = stage_graph.stage("manager", manager::stage);
    let processing_errors_stage = stage_graph
        .wire_up(processing_errors_stage, ())
        .without_state();
    let validation_errors_stage = stage_graph
        .wire_up(validation_errors_stage, manager_stage.sender())
        .without_state();

    let forward_chain_stage = stage_graph
        .wire_up(
            forward_chain_stage,
            (
                our_tip,
                manager_stage.sender(),
                validation_errors_stage.clone(),
                processing_errors_stage.clone(),
            ),
        )
        .without_state();

    let accept_stage = stage_graph
        .wire_up(
            accept_stage,
            accept::AcceptState::new(manager_stage.sender(), manager.config()),
        )
        .without_state();
    stage_graph
        .preload(&accept_stage, [PullAccept])
        .expect("preload should not fail on startup");

    let select_chain_stage = stage_graph
        .wire_up(
            select_chain_stage,
            (
                chain_selector,
                forward_chain_stage,
                validation_errors_stage.clone(),
            ),
        )
        .without_state();

    let validate_block_stage = stage_graph
        .wire_up(
            validate_block_stage,
            (
                select_chain_stage,
                validation_errors_stage.clone(),
                processing_errors_stage.clone(),
            ),
        )
        .without_state();

    let fetch_block_stage = stage_graph
        .wire_up(
            fetch_block_stage,
            (
                validate_block_stage,
                validation_errors_stage.clone(),
                processing_errors_stage.clone(),
                manager_stage.sender(),
            ),
        )
        .without_state();

    let validate_header_stage = stage_graph
        .wire_up(
            validate_header_stage,
            (fetch_block_stage, validation_errors_stage.clone()),
        )
        .without_state();

    let receive_header_stage = stage_graph
        .wire_up(
            receive_header_stage,
            receive_header::State::new(
                validate_header_stage,
                validation_errors_stage,
                processing_errors_stage,
            ),
        )
        .without_state();

    let pull_stage = stage_graph.stage("pull", pull::stage);
    let pull_stage = stage_graph
        .wire_up(pull_stage, (sync_tracker, receive_header_stage))
        .without_state();
    stage_graph
        .wire_up(manager_stage, (manager, pull_stage))
        .without_state()
}

/// Wrap a function taking `ConsensusEffects` so that it can be used in a stage graph that provides
/// the `Effects` type. The `ConsensusEffects` provide a higher-level API for executing external effects
/// during the consensus stages.
fn with_consensus_effects<Msg, St, F1, Fut>(
    mut f: F1,
) -> impl FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send
where
    F1: FnMut(St, Msg, ConsensusEffects<Msg>) -> Fut + 'static + Send,
    Fut: Future<Output = St> + 'static + Send,
    Msg: SendData + serde::de::DeserializeOwned + Sync + Clone,
    St: SendData,
{
    move |state, message, effects| f(state, message, ConsensusEffects::new(effects))
}
