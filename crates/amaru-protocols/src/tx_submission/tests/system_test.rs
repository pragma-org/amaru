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

use crate::{
    chainsync::{self, ChainSyncInitiatorMsg},
    connection::{self, ConnectionMessage},
    network_effects::create_connection,
    protocol::Role,
    store_effects::ResourceHeaderStore,
};
use amaru_kernel::{peer::Peer, protocol_messages::network_magic::NetworkMagic};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::{ConnectionResource, in_memory_consensus_store::InMemConsensusStore};
use pure_stage::{StageGraph, tokio::TokioBuilder};
use std::{sync::Arc, time::Duration};
use tokio::{runtime::Handle, time::timeout};
use tracing_subscriber::EnvFilter;

/// You can run this test against a real upstream node (don't forget to include `-- --ignored` in `cargo test`).
/// The upstream node must be either running at 127.0.0.1:3000 or at the address specified in the `PEER`
/// environment variable.
#[tokio::test]
#[ignore]
async fn test_tx_submission_with_node() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .init();

    let conn = TokioConnections::new(65535);
    let conn_id = create_connection(&conn).await?;

    let mut network = TokioBuilder::default();

    network
        .resources()
        .put::<ConnectionResource>(Arc::new(conn));
    network
        .resources()
        .put::<ResourceHeaderStore>(Arc::new(InMemConsensusStore::new()));

    let pipeline = network.stage(
        "pipeline",
        async |_st: (), msg: ChainSyncInitiatorMsg, eff| {
            use chainsync::InitiatorResult::*;
            match msg.msg {
                Initialize => {
                    tracing::info!(peer = %msg.peer,"initialize");
                }
                IntersectFound(point, tip) => {
                    tracing::info!(peer = %msg.peer, ?point, ?tip, "intersect found");
                }
                IntersectNotFound(tip) => {
                    tracing::info!(peer = %msg.peer, ?tip, "intersect not found");
                    eff.send(&msg.handler, chainsync::InitiatorMessage::Done)
                        .await;
                    return eff.terminate().await;
                }
                RollForward(header_content, tip) => {
                    tracing::info!(peer = %msg.peer, header_content.variant, ?header_content.byron_prefix, ?tip, "roll forward");
                    eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                        .await;
                }
                RollBackward(point, tip) => {
                    tracing::info!(peer = %msg.peer, ?point, ?tip, "roll backward");
                    eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                        .await;
                }
            };
        },
    );
    let pipeline = network.wire_up(pipeline, ());

    let connection = network.stage("connection", connection::stage);
    let connection = network.wire_up(
        connection,
        connection::Connection::new(
            Peer::new("upstream"),
            conn_id,
            Role::Initiator,
            NetworkMagic::for_testing(),
            pipeline.without_state(),
        ),
    );
    network
        .preload(connection, [ConnectionMessage::Initialize])
        .unwrap();

    let running = network.run(Handle::current());
    match timeout(Duration::from_secs(20), running.join()).await {
        Ok(_) => anyhow::bail!("test should have timed out"),
        Err(_) => {
            tracing::info!("test timed out as expected");
            Ok(())
        }
    }
}
