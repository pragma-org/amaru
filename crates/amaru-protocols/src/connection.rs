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

use crate::blockfetch::StreamBlocks;
use crate::manager::ManagerConfig;
use crate::{
    blockfetch::{
        self, BlockFetchMessage, Blocks, register_blockfetch_initiator,
        register_blockfetch_responder,
    },
    chainsync::{
        self, ChainSyncInitiatorMsg, register_chainsync_initiator, register_chainsync_responder,
    },
    handshake,
    keepalive::register_keepalive,
    mux::{self, HandlerMessage, MuxMessage},
    protocol::{Inputs, PROTO_HANDSHAKE, Role},
    protocol_messages::{
        handshake::HandshakeResult, version_data::VersionData, version_number::VersionNumber,
        version_table::VersionTable,
    },
    store_effects::Store,
    tx_submission::register_tx_submission,
};
use amaru_kernel::{EraHistory, NetworkMagic, ORIGIN_HASH, Peer, Point, Tip};
use amaru_ouroboros::{ConnectionId, ReadOnlyChainStore, TxOrigin};
use pure_stage::{Effects, StageRef, Void};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    params: Params,
    state: State,
}

impl Connection {
    pub fn new(
        peer: Peer,
        conn_id: ConnectionId,
        role: Role,
        config: ManagerConfig,
        magic: NetworkMagic,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
        era_history: Arc<EraHistory>,
    ) -> Self {
        Self {
            params: Params {
                peer,
                conn_id,
                role,
                config,
                magic,
                pipeline,
                era_history,
            },
            state: State::Initial,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Params {
    peer: Peer,
    conn_id: ConnectionId,
    role: Role,
    magic: NetworkMagic,
    config: ManagerConfig,
    pipeline: StageRef<ChainSyncInitiatorMsg>,
    era_history: Arc<EraHistory>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    Initial,
    Handshake {
        muxer: StageRef<MuxMessage>,
        handshake: StageRef<Inputs<Void>>,
    },
    Initiator(StateInitiator),
    Responder(StateResponder),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StateInitiator {
    chainsync_initiator: StageRef<chainsync::InitiatorMessage>,
    blockfetch_initiator: StageRef<blockfetch::BlockFetchMessage>,
    version_number: VersionNumber,
    version_data: VersionData,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    keepalive: StageRef<HandlerMessage>,
    tx_submission: StageRef<HandlerMessage>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StateResponder {
    chainsync_responder: StageRef<chainsync::ResponderMessage>,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    keepalive: StageRef<HandlerMessage>,
    tx_submission: StageRef<HandlerMessage>,
    blockfetch_responder: StageRef<StreamBlocks>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionMessage {
    Initialize,
    Disconnect,
    Handshake(HandshakeResult),
    FetchBlocks {
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
    },
    // LATER: make full duplex, etc.
}

pub async fn stage(
    Connection { params, state }: Connection,
    msg: ConnectionMessage,
    eff: Effects<ConnectionMessage>,
) -> Connection {
    let state = match (state, msg) {
        (_, ConnectionMessage::Disconnect) => return eff.terminate().await,
        (State::Initial, ConnectionMessage::Initialize) => do_initialize(&params, eff).await,
        (State::Handshake { muxer, handshake }, ConnectionMessage::Handshake(handshake_result)) => {
            do_handshake(
                &params,
                muxer,
                params.pipeline.clone(),
                handshake,
                handshake_result,
                eff,
            )
            .await
        }
        (State::Initiator(s), ConnectionMessage::FetchBlocks { from, through, cr }) => {
            eff.send(
                &s.blockfetch_initiator,
                BlockFetchMessage::RequestRange { from, through, cr },
            )
            .await;
            State::Initiator(s)
        }
        (
            state @ (State::Initial | State::Handshake { .. }),
            msg @ ConnectionMessage::FetchBlocks { .. },
        ) => {
            // The peer might be still connecting. In that case we reschedule the message
            // If the peer eventually can't be fully initialized, the caller timeout will trigger.
            // We schedule after the reconnect delay (2s by default) which is shorter than the call
            // timeout (5s) (whereas a full connection timeout is 10s).
            eff.schedule_after(msg, params.config.reconnect_delay).await;
            state
        }
        x => unimplemented!("{x:?}"),
    };
    Connection { params, state }
}

async fn do_initialize(
    Params {
        conn_id,
        role,
        magic,
        ..
    }: &Params,
    eff: Effects<ConnectionMessage>,
) -> State {
    let muxer = eff.stage("mux", mux::stage).await;
    let muxer = eff
        .wire_up(
            muxer,
            mux::State::new(*conn_id, &[(PROTO_HANDSHAKE.erase(), 5760)], *role),
        )
        .await;

    let handshake_result = eff
        .contramap(eff.me(), "handshake_result", ConnectionMessage::Handshake)
        .await;

    let handshake = match role {
        Role::Initiator => {
            eff.wire_up(
                eff.stage("handshake", handshake::initiator()).await,
                handshake::HandshakeInitiator::new(
                    muxer.clone(),
                    handshake_result,
                    VersionTable::v11_and_above(*magic, true),
                ),
            )
            .await
        }
        Role::Responder => {
            eff.wire_up(
                eff.stage("handshake", handshake::responder()).await,
                handshake::HandshakeResponder::new(
                    muxer.clone(),
                    handshake_result,
                    VersionTable::v11_and_above(*magic, true),
                ),
            )
            .await
        }
    };

    let handler = eff
        .contramap(&handshake, "handshake_bytes", Inputs::Network)
        .await;

    let protocol = match role {
        Role::Initiator => PROTO_HANDSHAKE.erase(),
        Role::Responder => PROTO_HANDSHAKE.responder().erase(),
    };
    eff.send(
        &muxer,
        MuxMessage::Register {
            protocol,
            frame: mux::Frame::OneCborItem,
            handler,
            max_buffer: 5760,
        },
    )
    .await;

    State::Handshake { muxer, handshake }
}

#[expect(clippy::expect_used)]
async fn do_handshake(
    Params {
        role,
        peer,
        conn_id,
        era_history,
        ..
    }: &Params,
    muxer: StageRef<MuxMessage>,
    pipeline: StageRef<ChainSyncInitiatorMsg>,
    handshake: StageRef<Inputs<Void>>,
    handshake_result: HandshakeResult,
    eff: Effects<ConnectionMessage>,
) -> State {
    let (version_number, version_data) = match handshake_result {
        HandshakeResult::Accepted(version_number, version_data) => (version_number, version_data),
        HandshakeResult::Refused(refuse_reason) => {
            tracing::error!(?refuse_reason, "handshake refused");
            return eff.terminate().await;
        }
        HandshakeResult::Query(version_table) => {
            tracing::info!(?version_table, "handshake query reply");
            return eff.terminate().await;
        }
    };

    let keepalive = register_keepalive(*role, muxer.clone(), &eff).await;
    let tx_submission =
        register_tx_submission(*role, muxer.clone(), &eff, TxOrigin::Remote(peer.clone())).await;

    if *role == Role::Initiator {
        let chainsync_initiator =
            register_chainsync_initiator(&muxer, peer.clone(), *conn_id, pipeline, &eff).await;
        let blockfetch_initiator = register_blockfetch_initiator(
            &muxer,
            peer.clone(),
            *conn_id,
            era_history.clone(),
            &eff,
        )
        .await;
        State::Initiator(StateInitiator {
            chainsync_initiator,
            blockfetch_initiator,
            version_number,
            version_data,
            muxer,
            handshake,
            keepalive,
            tx_submission,
        })
    } else {
        let store = Store::new(eff.clone());
        let upstream = store.get_best_chain_hash();
        let upstream = if upstream == ORIGIN_HASH {
            Tip::new(Point::Origin, 0.into())
        } else {
            let header = store
                .load_header(&upstream)
                .expect("best chain hash not found");
            header.tip()
        };
        let chainsync_responder =
            register_chainsync_responder(&muxer, upstream, peer.clone(), *conn_id, &eff).await;
        let blockfetch_responder = register_blockfetch_responder(&muxer, &eff).await;

        State::Responder(StateResponder {
            chainsync_responder,
            blockfetch_responder,
            muxer,
            handshake,
            keepalive,
            tx_submission,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::NetworkName;
    use pure_stage::{Effect, StageGraph, simulation::SimulationBuilder};

    #[test]
    fn test_fetch_blocks_in_initial_state_reschedules() {
        fetch_blocks_in_disconnected_state_reschedules(State::Initial);
    }

    #[test]
    fn test_fetch_blocks_in_handshake_state_reschedules() {
        let handshake_state = State::Handshake {
            muxer: StageRef::blackhole(),
            handshake: StageRef::blackhole(),
        };
        fetch_blocks_in_disconnected_state_reschedules(handshake_state);
    }

    // HELPERS

    fn test_connection(state: State) -> Connection {
        let era_history: &EraHistory = NetworkName::Preprod.into();
        Connection {
            params: Params {
                peer: Peer::new("test-peer"),
                conn_id: ConnectionId::initial(),
                role: Role::Initiator,
                config: ManagerConfig::default(),
                magic: NetworkMagic::PREPROD,
                pipeline: StageRef::blackhole(),
                era_history: Arc::new(era_history.clone()),
            },
            state,
        }
    }

    fn fetch_blocks_in_disconnected_state_reschedules(connection_state: State) {
        let mut network = SimulationBuilder::default();

        let connection_stage = network.stage("connection", stage);
        let connection_stage =
            network.wire_up(connection_stage, test_connection(connection_state.clone()));

        let (blocks_output, _rx) = network.output::<Blocks>("blocks_output", 10);

        let fetch_msg = ConnectionMessage::FetchBlocks {
            from: Point::Origin,
            through: Point::Origin,
            cr: blocks_output,
        };

        network.preload(&connection_stage, [fetch_msg]).unwrap();

        let mut running = network.run();
        let start_time = running.now();

        let stage_name = connection_stage.name().clone();
        running.breakpoint(
            "schedule",
            move |eff| matches!(eff, Effect::Schedule { at_stage, .. } if *at_stage == stage_name),
        );

        let effect = running.run_until_blocked().assert_breakpoint("schedule");

        let reconnect_delay = ManagerConfig::default().reconnect_delay;
        if let Effect::Schedule { id, .. } = &effect {
            let delay = id.time().checked_since(start_time).unwrap();
            assert!(delay >= reconnect_delay);
        } else {
            panic!("Expected Schedule effect");
        }

        // Clear the breakpoint before continuing
        running.clear_breakpoint("schedule");
        running.handle_effect(effect);

        // Let the simulation continue until blocked (will hit the scheduled wake up)
        running.run_until_sleeping_or_blocked().assert_sleeping();

        // Verify state remains the same
        let state = running.get_state(&connection_stage).unwrap();
        assert_eq!(state.state, connection_state);
    }
}
