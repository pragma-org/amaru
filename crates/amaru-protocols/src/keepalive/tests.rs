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
    connection::{self, ConnectionMessage},
    network_effects::create_connection,
    protocol::Role,
};
use amaru_kernel::{EraHistory, NetworkMagic, NetworkName, Peer};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::ConnectionResource;
use pure_stage::{StageGraph, StageRef, tokio::TokioBuilder};
use std::{sync::Arc, time::Duration};
use tokio::{runtime::Runtime, time::timeout};
use tracing_subscriber::EnvFilter;

#[test]
#[ignore]
fn test_keepalive_with_node() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    tracing::trace!("test");

    let rt = Runtime::new().unwrap();

    let conn = TokioConnections::new(65535);
    let conn_id = rt.block_on(create_connection(&conn)).unwrap();

    let mut network = TokioBuilder::default();

    network
        .resources()
        .put::<ConnectionResource>(Arc::new(conn));

    let era_history: &EraHistory = NetworkName::Preprod.into();
    let connection = network.stage("connection", connection::stage);
    let connection = network.wire_up(
        connection,
        connection::Connection::new(
            Peer::new("upstream"),
            conn_id,
            Role::Initiator,
            NetworkMagic::for_testing(),
            StageRef::blackhole(),
            Arc::new(era_history.clone()),
        ),
    );
    network
        .preload(connection, [ConnectionMessage::Initialize])
        .unwrap();

    let running = network.run(rt.handle().clone());
    match rt.block_on(async { timeout(Duration::from_secs(10), running.join()).await }) {
        Ok(_) => panic!("test should have timed out"),
        Err(_) => {
            tracing::info!("test timed out as expected");
        }
    }
}
