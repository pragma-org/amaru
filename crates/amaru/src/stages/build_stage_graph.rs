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

use std::{collections::BTreeSet, sync::Arc};

use amaru_consensus::stages::{
    adopt_chain::{self, AdoptChain},
    block_source::{self, BlockSource},
    fetch_blocks::{self, FetchBlocks, FetchBlocksMsg},
    mempool::{self, MempoolStageState},
    peer_selection::{self, PeerSelection, PeerSelectionMsg},
    select_chain::{self, SelectChain, SelectChainMsg},
    track_peers::{self, TrackPeers, TrackPeersMsg},
    validate_block::{self, ValidateBlock, ValidateBlockMsg},
};
use amaru_kernel::{BlockHeader, EraHistory, GlobalParameters, HeaderHash, Peer, Point, Tip};
use amaru_ouroboros::MempoolMsg;
use amaru_protocols::{
    manager,
    manager::{Manager, ManagerConfig, ManagerMessage, PeerSelectionNotify},
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
    our_candidate: Option<(BlockHeader, Vec<HeaderHash>)>,
    stage_graph: &mut impl StageGraph,
) -> NodeStages {
    let manager = stage_graph.stage("manager", manager::stage);
    let peer_selection = stage_graph.stage("peer_selection", peer_selection::stage);
    let peer_selection_ref = peer_selection.sender();

    let static_peers: BTreeSet<Peer> = config.upstream_peers.iter().map(|s| Peer::new(s)).collect();
    let peer_selection = stage_graph
        .wire_up(peer_selection, PeerSelection::new(manager.sender(), static_peers, config.peer_removal_cooldown_secs));

    let peer_selection_notify =
        stage_graph.contramap(&peer_selection_ref, "peer_selection_notify", |n: PeerSelectionNotify| match n {
            PeerSelectionNotify::DownstreamConnected(p) => PeerSelectionMsg::DownstreamConnected(p),
        });

    let track_peers = stage_graph.stage("track_peers", track_peers::stage);
    let select_chain = stage_graph.stage("select_chain", select_chain::stage);
    let fetch_blocks = stage_graph.stage("fetch_blocks", fetch_blocks::stage);
    let validate_block = stage_graph.stage("validate_block", validate_block::stage);
    let adopt_chain = stage_graph.stage("adopt_chain", adopt_chain::stage);
    let mempool_stage = stage_graph.stage("mempool", mempool::stage);
    let block_source_stage = stage_graph.stage("block_source", block_source::stage);
    let block_source_sender = block_source_stage.sender();

    let k = {
        #[expect(clippy::expect_used)]
        global_parameters
            .consensus_security_param
            .try_into()
            .expect("consensus security param will not be larger than u64::MAX")
    };
    let _block_source = stage_graph.wire_up(
        block_source_stage,
        BlockSource::new(ledger_tip, config.block_source_max_tip_distance, peer_selection_ref.clone()),
    );

    let adopt_chain =
        stage_graph.wire_up(adopt_chain, AdoptChain::new(manager.sender(), block_source_sender.clone(), k, ledger_tip));

    let validate_block = stage_graph.wire_up(
        validate_block,
        ValidateBlock::new(
            adopt_chain.without_state(),
            select_chain.sender(),
            block_source_sender.clone(),
            ledger_tip.point(),
        ),
    );
    let validate_block_input =
        stage_graph.contramap(validate_block, "validate_block_input", |(tip, parent, max_block_height)| {
            ValidateBlockMsg::new(tip, parent, max_block_height)
        });

    let fetch_blocks = stage_graph.wire_up(
        fetch_blocks,
        FetchBlocks::new(validate_block_input, select_chain.sender(), manager.sender(), block_source_sender),
    );
    let fetch_blocks_input =
        stage_graph.contramap(fetch_blocks, "fetch_blocks_input", |(tip, parent)| FetchBlocksMsg::NewTip(tip, parent));

    let select_chain = stage_graph.wire_up(select_chain, SelectChain::new(fetch_blocks_input, our_candidate));
    #[expect(clippy::expect_used)]
    stage_graph
        .preload(&select_chain, [SelectChainMsg::FetchNextFrom(Point::Origin)])
        .expect("initialization message must be preloaded");
    let select_chain_input = stage_graph
        .contramap(select_chain, "select_chain_input", |(tip, parent)| SelectChainMsg::TipFromUpstream(tip, parent));

    let track_peers = stage_graph.wire_up(
        track_peers,
        TrackPeers::new(era_history.clone(), peer_selection_ref, select_chain_input, k, config.defer_req_next_poll_ms),
    );
    let track_peers_input = stage_graph.contramap(track_peers, "track_peers_input", TrackPeersMsg::FromUpstream);

    #[expect(clippy::expect_used)]
    stage_graph
        .preload(&peer_selection, [PeerSelectionMsg::ConnectInitial])
        .expect("initialization message must be preloaded");

    let mempool_stage = stage_graph.wire_up(mempool_stage, MempoolStageState::default()).without_state();

    let manager_stage = stage_graph
        .wire_up(
            manager,
            Manager::new(
                config.network_magic,
                ManagerConfig::default(),
                Arc::new(era_history.clone()),
                track_peers_input,
                mempool_stage.clone(),
                peer_selection_notify,
            ),
        )
        .without_state();
    NodeStages { manager_stage, mempool_stage }
}

/// This data types encapsulates stage references that we need to export in order to
/// interact with some stages of the processing graph.
#[derive(Debug, Clone)]
pub struct NodeStages {
    pub manager_stage: StageRef<ManagerMessage>,
    pub mempool_stage: StageRef<MempoolMsg>,
}

impl NodeStages {
    pub fn manager_stage(&self) -> StageRef<ManagerMessage> {
        self.manager_stage.clone()
    }

    pub fn mempool_stage(&self) -> StageRef<MempoolMsg> {
        self.mempool_stage.clone()
    }
}
