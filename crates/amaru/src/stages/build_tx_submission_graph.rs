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

use amaru_consensus::consensus::stages::receive_tx_reply::Servers;
use amaru_consensus::consensus::stages::receive_tx_request::Clients;
use amaru_consensus::consensus::stages::{receive_tx_reply, receive_tx_request};
use amaru_consensus::consensus::{effects::ConsensusEffects, errors::ProcessingFailed};
use amaru_kernel::TxClientReply;
use amaru_kernel::tx_submission_events::TxServerRequest;
use amaru_network::tx_submission::{Blocking, ServerParams};
use pure_stage::{Effects, SendData, StageGraph, StageRef};

/// Create the graph of stages supporting the tx submission protocol.
pub fn build_tx_submission_graph(
    network: &mut impl StageGraph,
) -> (StageRef<TxServerRequest>, StageRef<TxClientReply>) {
    let receive_tx_request_stage = network.stage(
        "receive_tx_request",
        with_tx_effects(receive_tx_request::stage),
    );
    let receive_tx_reply_stage =
        network.stage("receive_tx_reply", with_tx_effects(receive_tx_reply::stage));

    let processing_errors_stage = network.stage(
        "processing_errors",
        async |_, error: ProcessingFailed, eff| {
            tracing::error!(%error, "stage error");
            // termination here will tear down the entire stage graph
            eff.terminate().await
        },
    );
    let processing_errors_stage = network.wire_up(processing_errors_stage, ());

    let receive_tx_request_stage = network.wire_up(
        receive_tx_request_stage,
        (
            Clients::new(),
            processing_errors_stage.clone().without_state(),
        ),
    );

    let receive_tx_reply_stage = network.wire_up(
        receive_tx_reply_stage,
        (
            Servers::new(ServerParams::new(100, 100, Blocking::Yes)),
            processing_errors_stage.without_state(),
        ),
    );

    (
        receive_tx_request_stage.without_state(),
        receive_tx_reply_stage.without_state(),
    )
}

pub fn with_tx_effects<Msg, St, F1, Fut>(
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
