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

use amaru_consensus::consensus::effects::ConsensusEffects;
use amaru_consensus::consensus::errors::{ProcessingFailed, ValidationFailed};
use amaru_consensus::consensus::events::ChainSyncEvent;
use amaru_consensus::consensus::stages::select_chain::SelectChain;
use amaru_consensus::consensus::stages::track_peers::SyncTracker;
use amaru_consensus::consensus::stages::{
    fetch_block, forward_chain, receive_header, select_chain, track_peers, validate_block,
    validate_header,
};
use amaru_consensus::consensus::tip::HeaderTip;
use amaru_kernel::protocol_parameters::{ConsensusParameters, GlobalParameters};
use amaru_slot_arithmetic::EraHistory;
use pure_stage::{Effects, SendData, StageGraph, StageRef};
use std::sync::Arc;

/// Create the graph of stages supporting the consensus protocol.
/// The output of the graph is passed as a parameter, allowing the caller to
/// decide what to do with the results the graph processing.
pub fn build_stage_graph(
    global_parameters: &GlobalParameters,
    era_history: &'static EraHistory,
    chain_selector: SelectChain,
    sync_tracker: SyncTracker,
    our_tip: HeaderTip,
    network: &mut impl StageGraph,
) -> StageRef<ChainSyncEvent> {
    let receive_header_stage = network.stage(
        "receive_header",
        with_consensus_effects(receive_header::stage),
    );
    let track_peers_stage =
        network.stage("track_peers", with_consensus_effects(track_peers::stage));
    let validate_header_stage = network.stage(
        "validate_header",
        with_consensus_effects(validate_header::stage),
    );
    let fetch_block_stage =
        network.stage("fetch_block", with_consensus_effects(fetch_block::stage));
    let validate_block_stage = network.stage(
        "validate_block",
        with_consensus_effects(validate_block::stage),
    );
    let select_chain_stage =
        network.stage("select_chain", with_consensus_effects(select_chain::stage));
    let forward_chain_stage = network.stage(
        "forward_chain",
        with_consensus_effects(forward_chain::stage),
    );

    // TODO: currently only validate_header errors, will need to grow into all error handling
    let validation_errors_stage = network.stage(
        "validation_errors",
        async |_, error: ValidationFailed, eff| {
            tracing::error!(%error, "stage error");
            // TODO: implement specific actions once we have an upstream network
            // termination here will tear down the entire stage graph
            eff.terminate().await
        },
    );

    let processing_errors_stage = network.stage(
        "processing_errors",
        async |_, error: ProcessingFailed, eff| {
            tracing::error!(%error, "stage error");
            // termination here will tear down the entire stage graph
            eff.terminate().await
        },
    );

    let validation_errors_stage = network.wire_up(validation_errors_stage, ());
    let processing_errors_stage = network.wire_up(processing_errors_stage, ());

    let forward_chain_stage = network.wire_up(
        forward_chain_stage,
        (
            our_tip,
            validation_errors_stage.clone().without_state(),
            processing_errors_stage.clone().without_state(),
        ),
    );
    let select_chain_stage = network.wire_up(
        select_chain_stage,
        (
            chain_selector,
            forward_chain_stage.without_state(),
            validation_errors_stage.clone().without_state(),
        ),
    );
    let validate_block_stage = network.wire_up(
        validate_block_stage,
        (
            select_chain_stage.without_state(),
            validation_errors_stage.clone().without_state(),
            processing_errors_stage.clone().without_state(),
        ),
    );
    let fetch_block_stage = network.wire_up(
        fetch_block_stage,
        (
            validate_block_stage.clone().without_state(),
            validation_errors_stage.clone().without_state(),
            processing_errors_stage.clone().without_state(),
        ),
    );
    let validate_header_stage = network.wire_up(
        validate_header_stage,
        (
            Arc::new(ConsensusParameters::new(
                global_parameters.clone(),
                era_history,
                Default::default(),
            )),
            fetch_block_stage.without_state(),
            validation_errors_stage.clone().without_state(),
        ),
    );
    let track_peers_stage = network.wire_up(track_peers_stage, sync_tracker);
    let receive_header_stage = network.wire_up(
        receive_header_stage,
        (
            validate_header_stage.without_state(),
            track_peers_stage.without_state(),
            validation_errors_stage.without_state(),
            processing_errors_stage.without_state(),
        ),
    );

    receive_header_stage.without_state()
}

/// Wrap a function taking `ConsensusEffects` so that it can be used in a stage graph that provides
/// the `Effects` type. The `ConsensusEffects` provide a higher-level API for executing external effects
/// during the consensus stages.
///
/// Note: the EraHistory reference must be passed to be able to build a ChainStore that can reference
/// the correct era history. That ChainStore is returned by the ConsensusEffects::store() function.
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
