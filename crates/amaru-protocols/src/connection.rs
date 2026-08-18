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

use std::sync::Arc;

use amaru_kernel::{EraHistory, NetworkMagic, Peer, Point};
use amaru_observability::{Instrument, TraceContext, debug_span, error, info};
use amaru_ouroboros::{ConnectionId, MempoolMsg, TxOrigin};
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void, register_data_deserializer};

use crate::{
    blockfetch::{
        self, BlockFetchMessage, Blocks, StreamBlocks, register_blockfetch_initiator, register_blockfetch_responder,
    },
    chainsync::{
        self, ChainSyncInitiatorMsg, InitiatorResult, register_chainsync_initiator, register_chainsync_responder,
    },
    handshake,
    keepalive::register_keepalive,
    manager::{ManagerConfig, ManagerMessage},
    mux::{self, HandlerMessage, MuxMessage},
    peer_sharing::{PeerSharingMessage, ShareResult, register_peer_sharing_initiator, register_peer_sharing_responder},
    protocol::{Inputs, PROTO_HANDSHAKE, Role},
    protocol_messages::{
        handshake::HandshakeResult, version_data::VersionData, version_number::VersionNumber,
        version_table::VersionTable,
    },
    store_effects::Store,
    tx_submission::register_tx_submission,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    params: Params,
    state: State,
}

impl Connection {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        peer: Peer,
        conn_id: ConnectionId,
        role: Role,
        config: ManagerConfig,
        magic: NetworkMagic,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
        era_history: Arc<EraHistory>,
        mempool_stage: StageRef<MempoolMsg>,
        manager: StageRef<ManagerMessage>,
    ) -> Self {
        Self {
            params: Params { peer, conn_id, role, config, magic, pipeline, era_history, mempool_stage, manager },
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
    mempool_stage: StageRef<MempoolMsg>,
    manager: StageRef<ManagerMessage>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    Initial,
    Handshake { muxer: StageRef<MuxMessage>, handshake: StageRef<Inputs<Void>> },
    Initiator(StateInitiator),
    Responder(StateResponder),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StateInitiator {
    chainsync_initiator: StageRef<chainsync::InitiatorMessage>,
    blockfetch_initiator: StageRef<blockfetch::BlockFetchMessage>,
    peer_sharing_initiator: StageRef<PeerSharingMessage>,
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
    peer_sharing_responder: StageRef<crate::peer_sharing::ResponderMessage>,
}

/// Identity of a supervised child stage of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChildId {
    Mux,
    Handshake,
    KeepAlive,
    TxSubmission,
    ChainSync,
    BlockFetch,
    PeerSharing,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionMessage {
    Initialize,
    Disconnect,
    Handshake(HandshakeResult),
    FetchBlocks {
        from: Point,
        through: Point,
        id: u64,
        cr: StageRef<Blocks>,
    },
    /// Start periodic peer-sharing requests on this connection's initiator.
    RequestSharePeers {
        amount: u8,
        initial_delay: std::time::Duration,
        interval: std::time::Duration,
        reply_to: StageRef<ShareResult>,
    },
    NewTip(Point, TraceContext),
    /// A supervised mini-protocol or mux stage terminated.
    ChildDied(ChildId),
}

impl ConnectionMessage {
    fn message_type(&self) -> &'static str {
        match self {
            ConnectionMessage::Initialize => "Initialize",
            ConnectionMessage::Disconnect => "Disconnect",
            ConnectionMessage::Handshake(_) => "Handshake",
            ConnectionMessage::FetchBlocks { .. } => "FetchBlocks",
            ConnectionMessage::RequestSharePeers { .. } => "RequestSharePeers",
            ConnectionMessage::NewTip(_, _) => "NewTip",
            ConnectionMessage::ChildDied(_) => "ChildDied",
        }
    }

    pub fn new_tip(tip: Point) -> Self {
        ConnectionMessage::NewTip(tip, TraceContext::none())
    }
}

pub async fn stage(
    Connection { params, state }: Connection,
    msg: ConnectionMessage,
    eff: Effects<ConnectionMessage>,
) -> Connection {
    let message_type = msg.message_type().to_string();
    let Params { conn_id, role, .. } = params;
    let peer = params.peer.clone();

    async move {
        let state = match (state, msg) {
            (state, ConnectionMessage::Disconnect) => {
                return teardown(state, &params, &eff).await;
            }
            (state, ConnectionMessage::ChildDied(child)) => {
                info!(
                    protocols::connection::CHILD_DIED,
                    peer = &params.peer,
                    conn_id = conn_id.as_u64(),
                    child = format!("{child:?}")
                );
                return teardown(state, &params, &eff).await;
            }
            (State::Initial, ConnectionMessage::Initialize) => do_initialize(&params, eff).await,
            (State::Handshake { muxer, handshake }, ConnectionMessage::Handshake(handshake_result)) => {
                do_handshake(&params, muxer, params.pipeline.clone(), handshake, handshake_result, eff).await
            }
            (State::Initiator(s), ConnectionMessage::FetchBlocks { from, through, id, cr }) => {
                eff.send(&s.blockfetch_initiator, BlockFetchMessage::RequestRange { from, through, id, cr }).await;
                State::Initiator(s)
            }
            (
                State::Initiator(s),
                ConnectionMessage::RequestSharePeers { amount, initial_delay, interval, reply_to },
            ) => {
                eff.send(
                    &s.peer_sharing_initiator,
                    PeerSharingMessage::Start { amount, initial_delay, interval, reply_to },
                )
                .await;
                State::Initiator(s)
            }
            (State::Responder(s), ConnectionMessage::NewTip(tip, trace_context)) => {
                eff.send(&s.chainsync_responder, chainsync::ResponderMessage::NewTip(tip, trace_context)).await;
                State::Responder(s)
            }
            (State::Initiator(s), ConnectionMessage::NewTip(_, _)) => {
                // don't propagate new tip messages when using the initiator side of a connection.
                State::Initiator(s)
            }
            (state @ (State::Initial | State::Handshake { .. }), msg @ ConnectionMessage::FetchBlocks { .. }) => {
                // The peer might be still connecting. In that case we reschedule the message
                // If the peer eventually can't be fully initialized, the caller timeout will trigger.
                // We schedule after the reconnect delay (2s by default) which is shorter than the call
                // timeout (5s) (whereas a full connection timeout is 10s).
                eff.schedule_after(msg, params.config.reconnect_delay).await;
                state
            }
            (state @ (State::Initial | State::Handshake { .. }), msg @ ConnectionMessage::RequestSharePeers { .. }) => {
                eff.schedule_after(msg, params.config.reconnect_delay).await;
                state
            }
            (state @ (State::Initial | State::Handshake { .. }), msg @ ConnectionMessage::NewTip(_, _)) => {
                // The peer might be still connecting. Reschedule the NewTip message.
                eff.schedule_after(msg, params.config.reconnect_delay).await;
                state
            }
            x => unimplemented!("{x:?}"),
        };
        Connection { params, state }
    }
    .instrument(debug_span!(
        protocols::connection::message::PROCESS,
        message_type = message_type,
        conn_id = conn_id.as_u64(),
        peer = peer,
        role = role.to_string(),
    ))
    .await
}

/// Notify track_peers that the initiator chainsync session ended, then terminate this connection.
///
/// Parent termination aborts children without delivering their tombstones, so the chainsync
/// purge signal must be sent explicitly here whenever an initiator session may have been started.
async fn teardown(state: State, params: &Params, eff: &Effects<ConnectionMessage>) -> Connection {
    match state {
        State::Initiator(..) => {
            notify_chainsync_terminated(params, eff).await;
        }
        State::Initial | State::Handshake { .. } | State::Responder(_) => {}
    }
    eff.terminate().await
}

async fn notify_chainsync_terminated(params: &Params, eff: &Effects<ConnectionMessage>) {
    eff.send(
        &params.pipeline,
        ChainSyncInitiatorMsg {
            peer: params.peer.clone(),
            conn_id: params.conn_id,
            handler: StageRef::blackhole(),
            msg: InitiatorResult::Terminated,
        },
    )
    .await;
}

async fn do_initialize(Params { conn_id, role, magic, peer, .. }: &Params, eff: Effects<ConnectionMessage>) -> State {
    let muxer = eff.stage("mux", mux::stage).await;
    let muxer = eff.supervise(muxer, ConnectionMessage::ChildDied(ChildId::Mux));
    let muxer =
        eff.wire_up(muxer, mux::State::new(*conn_id, &[(PROTO_HANDSHAKE.erase(), 5760)], *role, peer.clone())).await;

    let handshake_result = eff.me_ref().contramap(ConnectionMessage::Handshake);

    let handshake = match role {
        Role::Initiator => {
            let hs = eff.stage("handshake", handshake::initiator()).await;
            let hs = eff.supervise(hs, ConnectionMessage::ChildDied(ChildId::Handshake));
            eff.wire_up(
                hs,
                handshake::HandshakeInitiator::new(
                    muxer.clone(),
                    handshake_result,
                    VersionTable::v11_and_above(*magic, true, true),
                ),
            )
            .await
        }
        Role::Responder => {
            let hs = eff.stage("handshake", handshake::responder()).await;
            let hs = eff.supervise(hs, ConnectionMessage::ChildDied(ChildId::Handshake));
            eff.wire_up(
                hs,
                handshake::HandshakeResponder::new(
                    muxer.clone(),
                    handshake_result,
                    // Use initiator_only_diffusion_mode = false so downstream peers
                    // know we can serve as chainsync/blockfetch server
                    VersionTable::v11_and_above(*magic, false, true),
                ),
            )
            .await
        }
    };

    let handler = handshake.contramap(Inputs::Network);

    let protocol = match role {
        Role::Initiator => PROTO_HANDSHAKE.erase(),
        Role::Responder => PROTO_HANDSHAKE.responder().erase(),
    };
    eff.send(&muxer, MuxMessage::Register { protocol, frame: mux::Frame::OneCborItem, handler, max_buffer: 5760 })
        .await;

    State::Handshake { muxer, handshake }
}

async fn do_handshake(
    Params { role, peer, conn_id, manager, era_history, mempool_stage, config, .. }: &Params,
    muxer: StageRef<MuxMessage>,
    pipeline_ref: StageRef<ChainSyncInitiatorMsg>,
    handshake: StageRef<Inputs<Void>>,
    handshake_result: HandshakeResult,
    eff: Effects<ConnectionMessage>,
) -> State {
    let (version_number, version_data) = match handshake_result {
        HandshakeResult::Accepted(version_number, version_data) => (version_number, version_data),
        HandshakeResult::Refused(refuse_reason) => {
            error!(protocols::connection::HANDSHAKE_REFUSED, reason = format!("{refuse_reason:?}"));
            return eff.terminate().await;
        }
        HandshakeResult::Query(version_table) => {
            info!(protocols::connection::HANDSHAKE_QUERY_REPLY, version_table = format!("{version_table:?}"));
            return eff.terminate().await;
        }
    };

    let full_duplex_capable = version_data.is_full_duplex_capable();
    // TODO: this needs to change once we actually start supporting full duplex mode
    let full_duplex = false;
    let advertisable = version_data.is_advertisable();

    eff.send(
        manager,
        ManagerMessage::HandshakeComplete {
            peer: peer.clone(),
            stage: eff.me(),
            conn_id: *conn_id,
            role: *role,
            full_duplex_capable,
            full_duplex,
            advertisable,
        },
    )
    .await;

    let keepalive = register_keepalive(
        *role,
        peer.clone(),
        *conn_id,
        muxer.clone(),
        &eff,
        ConnectionMessage::ChildDied(ChildId::KeepAlive),
    )
    .await;
    let tx_submission = register_tx_submission(
        *role,
        peer.clone(),
        muxer.clone(),
        &eff,
        TxOrigin::Remote(peer.clone()),
        mempool_stage.clone(),
        config.tx_submission_params,
        era_history.clone(),
        ConnectionMessage::ChildDied(ChildId::TxSubmission),
    )
    .await;

    if *role == Role::Initiator {
        let chainsync_initiator = register_chainsync_initiator(
            &muxer,
            peer.clone(),
            *conn_id,
            pipeline_ref,
            &eff,
            ConnectionMessage::ChildDied(ChildId::ChainSync),
        )
        .await;
        let blockfetch_initiator = register_blockfetch_initiator(
            &muxer,
            peer.clone(),
            *conn_id,
            &eff,
            ConnectionMessage::ChildDied(ChildId::BlockFetch),
        )
        .await;
        let peer_sharing_initiator = register_peer_sharing_initiator(
            &muxer,
            peer.clone(),
            *conn_id,
            &eff,
            ConnectionMessage::ChildDied(ChildId::PeerSharing),
        )
        .await;
        State::Initiator(StateInitiator {
            chainsync_initiator,
            blockfetch_initiator,
            peer_sharing_initiator,
            version_number,
            version_data,
            muxer,
            handshake,
            keepalive,
            tx_submission,
        })
    } else {
        let store = Store::new(eff.clone());
        let upstream = store.get_best_chain_tip().await;
        let chainsync_responder = register_chainsync_responder(
            &muxer,
            upstream,
            peer.clone(),
            *conn_id,
            &eff,
            ConnectionMessage::ChildDied(ChildId::ChainSync),
        )
        .await;
        let blockfetch_responder =
            register_blockfetch_responder(&muxer, &eff, ConnectionMessage::ChildDied(ChildId::BlockFetch)).await;
        let peer_sharing_responder = register_peer_sharing_responder(
            &muxer,
            peer.clone(),
            manager.clone(),
            &eff,
            ConnectionMessage::ChildDied(ChildId::PeerSharing),
        )
        .await;

        State::Responder(StateResponder {
            chainsync_responder,
            blockfetch_responder,
            peer_sharing_responder,
            muxer,
            handshake,
            keepalive,
            tx_submission,
        })
    }
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        register_data_deserializer::<(ConnectionId, StageRef<mux::MuxMessage>, Role)>().boxed(),
        register_data_deserializer::<Connection>().boxed(),
        register_data_deserializer::<ConnectionMessage>().boxed(),
    ]
}

#[cfg(test)]
mod tests {
    use amaru_kernel::PREPROD_ERA_HISTORY;
    use amaru_pure_stage::{Effect, StageGraph, simulation::SimulationBuilder};
    use tokio::runtime::Runtime;

    use super::*;

    #[test]
    fn test_fetch_blocks_in_initial_state_reschedules() {
        fetch_blocks_in_disconnected_state_reschedules(State::Initial);
    }

    #[test]
    fn test_fetch_blocks_in_handshake_state_reschedules() {
        let handshake_state = State::Handshake { muxer: StageRef::blackhole(), handshake: StageRef::blackhole() };
        fetch_blocks_in_disconnected_state_reschedules(handshake_state);
    }

    #[test]
    fn test_new_tip_in_initial_state_reschedules() {
        new_tip_in_disconnected_state_reschedules(State::Initial);
    }

    #[test]
    fn test_new_tip_in_handshake_state_reschedules() {
        let handshake_state = State::Handshake { muxer: StageRef::blackhole(), handshake: StageRef::blackhole() };
        new_tip_in_disconnected_state_reschedules(handshake_state);
    }

    fn fetch_blocks_in_disconnected_state_reschedules(connection_state: State) {
        assert_message_reschedules_in_disconnected_state(connection_state, |network| {
            let (blocks_output, _rx) = network.output::<Blocks>("blocks_output", 10);
            ConnectionMessage::FetchBlocks { from: Point::Origin, through: Point::Origin, id: 0, cr: blocks_output }
        });
    }

    fn new_tip_in_disconnected_state_reschedules(connection_state: State) {
        assert_message_reschedules_in_disconnected_state(connection_state, |_| {
            ConnectionMessage::new_tip(Point::Origin)
        });
    }

    fn assert_message_reschedules_in_disconnected_state(
        connection_state: State,
        make_msg: impl FnOnce(&mut SimulationBuilder) -> ConnectionMessage,
    ) {
        let mut network = SimulationBuilder::default();

        let connection_stage = network.stage("connection", stage);
        let connection_stage = network.wire_up(connection_stage, test_connection(connection_state.clone()));

        let msg = make_msg(&mut network);
        network.preload(&connection_stage, [msg]).unwrap();

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
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

    // HELPERS

    fn test_connection(state: State) -> Connection {
        Connection {
            params: Params {
                peer: Peer::new("test-peer"),
                conn_id: ConnectionId::initial(),
                role: Role::Initiator,
                config: ManagerConfig::default(),
                magic: NetworkMagic::PREPROD,
                pipeline: StageRef::blackhole(),
                era_history: Arc::new(PREPROD_ERA_HISTORY.clone()),
                mempool_stage: StageRef::blackhole(),
                manager: StageRef::blackhole(),
            },
            state,
        }
    }
}
