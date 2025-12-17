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
    bytes::NonEmptyBytes,
    handshake::{self, Role, messages::VersionTable},
    mux::{self, HandlerMessage, MuxMessage},
    protocol::PROTO_HANDSHAKE,
    socket::ConnectionResource,
    socket_addr::ToSocketAddrs,
};
use amaru_kernel::protocol_messages::{
    network_magic::NetworkMagic,
    version_data::{PEER_SHARING_DISABLED, VersionData},
    version_number::VersionNumber,
};
use futures_util::StreamExt;
use pure_stage::{
    Effect, StageGraph, simulation::SimulationBuilder, tokio::TokioBuilder,
    trace_buffer::TraceBuffer,
};
use std::{env, time::Duration};
use tokio::{runtime::Runtime, time::timeout};
use tracing_subscriber::EnvFilter;

#[test]
#[ignore]
fn test_against_node() {
    let _guard = pure_stage::register_data_deserializer::<NonEmptyBytes>();
    let _guard = pure_stage::register_data_deserializer::<handshake::HandshakeMessage>();
    let _guard = pure_stage::register_data_deserializer::<handshake::HandshakeResult>();
    let _guard = pure_stage::register_data_deserializer::<handshake::Handshake>();
    let _guard = pure_stage::register_data_deserializer::<mux::State>();
    let _guard = pure_stage::register_data_deserializer::<mux::MuxMessage>();
    let _guard = pure_stage::register_effect_deserializer::<crate::effects::RecvEffect>();
    let _guard = pure_stage::register_effect_deserializer::<crate::effects::SendEffect>();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    let rt = Runtime::new().unwrap();

    let conn = ConnectionResource::new(65535);
    let conn_id = rt
        .block_on(async {
            timeout(Duration::from_secs(5), async {
                let addr = ToSocketAddrs::String(
                    env::var("PEER").unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
                )
                .resolve()
                .await
                .unwrap();
                conn.connect(addr).await.unwrap()
            })
            .await
        })
        .unwrap();

    let trace_buffer = TraceBuffer::new_shared(1000, 1000000);
    let _guard = TraceBuffer::drop_guard(&trace_buffer);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);

    network.resources().put(conn);

    let mux = network.stage("mux", mux::stage);
    let mux = network.wire_up(mux, mux::State::new(conn_id, &[]));

    let (output, mut rx) = network.output::<handshake::HandshakeResult>("handshake_result", 10);

    let handshake = network.stage("handshake", handshake::stage);
    let handshake = network.wire_up(
        handshake,
        handshake::Handshake::new(
            mux.clone().without_state(),
            output.clone(),
            Role::Initiator,
            VersionTable::v11_and_above(NetworkMagic::MAINNET, true),
        ),
    );

    let handshake_bytes =
        network.contramap(
            handshake,
            "handshake_bytes",
            |msg: HandlerMessage| match msg {
                HandlerMessage::FromNetwork(bytes) => {
                    handshake::HandshakeMessage::FromNetwork(bytes)
                }
                HandlerMessage::Registered(_) => handshake::HandshakeMessage::Registered,
            },
        );

    network
        .preload(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_HANDSHAKE.erase(),
                frame: mux::Frame::OneCborItem,
                handler: handshake_bytes,
                max_buffer: 5760,
            }],
        )
        .unwrap();

    let mut running = network.run();

    running.breakpoint(
        "output",
        move |eff| matches!(eff, Effect::External { at_stage, .. } if at_stage == output.name()),
    );

    let output = running
        .run_until_blocked_incl_effects(rt.handle())
        .assert_breakpoint("output");
    running.handle_effect(output);
    rt.block_on(running.await_external_effect()).unwrap();
    let result = rx.try_next().unwrap();
    assert_eq!(
        result,
        handshake::HandshakeResult::Accepted(
            VersionNumber::V14,
            VersionData::new(NetworkMagic::MAINNET, true, PEER_SHARING_DISABLED, false),
        )
    );
}

#[test]
#[ignore]
fn test_against_node_with_tokio() {
    let _guard = pure_stage::register_data_deserializer::<NonEmptyBytes>();
    let _guard = pure_stage::register_data_deserializer::<handshake::HandshakeMessage>();
    let _guard = pure_stage::register_data_deserializer::<handshake::HandshakeResult>();
    let _guard = pure_stage::register_data_deserializer::<handshake::Handshake>();
    let _guard = pure_stage::register_data_deserializer::<mux::State>();
    let _guard = pure_stage::register_data_deserializer::<mux::MuxMessage>();
    let _guard = pure_stage::register_effect_deserializer::<crate::effects::RecvEffect>();
    let _guard = pure_stage::register_effect_deserializer::<crate::effects::SendEffect>();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    let rt = Runtime::new().unwrap();

    let conn = ConnectionResource::new(65535);
    let conn_id = rt
        .block_on(async {
            timeout(Duration::from_secs(5), async {
                let addr = ToSocketAddrs::String(
                    env::var("PEER").unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
                )
                .resolve()
                .await
                .unwrap();
                conn.connect(addr).await.unwrap()
            })
            .await
        })
        .unwrap();

    let trace_buffer = TraceBuffer::new_shared(1000, 1000000);
    let _guard = TraceBuffer::drop_guard(&trace_buffer);
    let mut network = TokioBuilder::default();

    network.resources().put(conn);

    let mux = network.stage("mux", mux::stage);
    let mux = network.wire_up(mux, mux::State::new(conn_id, &[]));

    let (output, mut rx) = network.output::<handshake::HandshakeResult>("handshake_result", 10);

    let handshake = network.stage("handshake", handshake::stage);
    let handshake = network.wire_up(
        handshake,
        handshake::Handshake::new(
            mux.clone().without_state(),
            output,
            Role::Initiator,
            VersionTable::v11_and_above(NetworkMagic::MAINNET, true),
        ),
    );

    let handshake_bytes =
        network.contramap(
            handshake,
            "handshake_bytes",
            |msg: HandlerMessage| match msg {
                HandlerMessage::FromNetwork(bytes) => {
                    handshake::HandshakeMessage::FromNetwork(bytes)
                }
                HandlerMessage::Registered(_) => handshake::HandshakeMessage::Registered,
            },
        );

    network
        .preload(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_HANDSHAKE.erase(),
                frame: mux::Frame::OneCborItem,
                handler: handshake_bytes,
                max_buffer: 5760,
            }],
        )
        .unwrap();

    let running = network.run(rt.handle().clone());

    let result = rt.block_on(rx.next()).unwrap();
    assert_eq!(
        result,
        handshake::HandshakeResult::Accepted(
            VersionNumber::V14,
            VersionData::new(NetworkMagic::MAINNET, true, PEER_SHARING_DISABLED, false),
        )
    );

    running.abort();
}
