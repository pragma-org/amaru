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

use std::sync::Arc;

use amaru_consensus::stages::{
    adopt_chain::{self, AdoptChain},
    fetch_blocks::{self, FetchBlocks, FetchBlocksMsg},
    select_chain_new::{self, SelectChain, SelectChainMsg},
    track_peers::{self, TrackPeers, TrackPeersMsg},
    validate_block2::{self, ValidateBlock, ValidateBlockMsg},
};
use amaru_kernel::{BlockHeader, EraHistory, GlobalParameters, Point, Tip};
use amaru_protocols::{
    manager,
    manager::{Manager, ManagerConfig, ManagerMessage},
};
use pure_stage::{StageGraph, StageRef};

use crate::stages::config::Config;

/// Create a graph of processing stages for the node.
///
/// Notes:
///
/// - The `manager::stage` will dynamically create stages when getting peer connections.
/// - Two stages are collecting failures and errors. According to <https://www.reactivemanifesto.org/glossary#Failure:>:
///     - "A failure is an unexpected event within a service that prevents it from continuing to function normally"
///     - "An error, ... is an expected and coded-for condition"
///
/// We terminate the node in case of a failure, while we just log errors.
///
pub fn build_stage_graph(
    config: &Config,
    era_history: &EraHistory,
    global_parameters: &GlobalParameters,
    ledger_tip: Tip,
    our_candidate: Option<BlockHeader>,
    stage_graph: &mut impl StageGraph,
) -> StageRef<ManagerMessage> {
    let manager = stage_graph.stage("manager", manager::stage);
    let track_peers = stage_graph.stage("track_peers", track_peers::stage);
    let select_chain = stage_graph.stage("select_chain", select_chain_new::stage);
    let fetch_blocks = stage_graph.stage("fetch_blocks", fetch_blocks::stage);
    let validate_block = stage_graph.stage("validate_block", validate_block2::stage);
    let adopt_chain = stage_graph.stage("adopt_chain", adopt_chain::stage);

    let k = {
        #[expect(clippy::unwrap_used)]
        global_parameters
            .consensus_security_param
            .try_into()
            .expect("consensus security param will not be larger than u64::MAX")
    };
    let adopt_chain = stage_graph.wire_up(adopt_chain, AdoptChain::new(manager.sender(), k, ledger_tip));

    let validate_block = stage_graph.wire_up(
        validate_block,
        ValidateBlock::new(adopt_chain.without_state(), select_chain.sender(), ledger_tip.point()),
    );
    let validate_block_input = stage_graph
        .contramap(validate_block, "validate_block_input", |(tip, parent)| ValidateBlockMsg::new(tip, parent));

    let fetch_blocks = stage_graph
        .wire_up(fetch_blocks, FetchBlocks::new(validate_block_input, select_chain.sender(), manager.sender()));
    let fetch_blocks_input =
        stage_graph.contramap(fetch_blocks, "fetch_blocks_input", |(tip, parent)| FetchBlocksMsg::NewTip(tip, parent));

    let select_chain = stage_graph.wire_up(select_chain, SelectChain::new(fetch_blocks_input, our_candidate));
    #[expect(clippy::expect_used)]
    stage_graph
        .preload(&select_chain, [SelectChainMsg::FetchNextFrom(Point::Origin)])
        .expect("initialization message must be preloaded");
    let select_chain_input = stage_graph
        .contramap(select_chain, "select_chain_input", |(tip, parent)| SelectChainMsg::TipFromUpstream(tip, parent));

    let track_peers =
        stage_graph.wire_up(track_peers, TrackPeers::new(era_history.clone(), manager.sender(), select_chain_input));
    let track_peers_input = stage_graph.contramap(track_peers, "track_peers_input", TrackPeersMsg::FromUpstream);

    stage_graph
        .wire_up(
            manager,
            Manager::new(
                config.network_magic,
                ManagerConfig::default(),
                Arc::new(era_history.clone()),
                track_peers_input,
            ),
        )
        .without_state()
}
