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
        self, BlockFetchMessage, Blocks, StreamBlocks, register_blockfetch_initiator,
        register_blockfetch_initiator_pipelined, register_blockfetch_responder,
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

/// Local use of a bearer: which initiator groups we intend to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum LocalUse {
    None,
    Maintenance,
    Diffusion,
}

impl LocalUse {
    pub fn default_for_role(role: Role) -> Self {
        match role {
            Role::Initiator => Self::Diffusion,
            Role::Responder => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    Initial,
    Handshake { muxer: StageRef<MuxMessage>, handshake: StageRef<Inputs<Void>> },
    Established(Established),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Established {
    desired_use: LocalUse,
    actual_use: LocalUse,
    version_number: VersionNumber,
    version_data: VersionData,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    keepalive: StageRef<HandlerMessage>,
    tx_submission: StageRef<HandlerMessage>,
    chainsync_initiator: Option<StageRef<chainsync::InitiatorMessage>>,
    blockfetch_initiator: Option<StageRef<blockfetch::BlockFetchMessage>>,
    peer_sharing_initiator: Option<StageRef<PeerSharingMessage>>,
    chainsync_responder: Option<StageRef<chainsync::ResponderMessage>>,
    blockfetch_responder: Option<StageRef<StreamBlocks>>,
    peer_sharing_responder: Option<StageRef<crate::peer_sharing::ResponderMessage>>,
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
    /// Peer selection (or default after handshake) wants this local use.
    SetLocalUse(LocalUse),
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
            ConnectionMessage::SetLocalUse(_) => "SetLocalUse",
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
    let peer = params.peer;

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
            (State::Established(s), ConnectionMessage::FetchBlocks { from, through, id, cr }) => {
                if let Some(blockfetch) = &s.blockfetch_initiator {
                    eff.send(blockfetch, BlockFetchMessage::RequestRange { from, through, id, cr }).await;
                }
                State::Established(s)
            }
            (
                State::Established(s),
                ConnectionMessage::RequestSharePeers { amount, initial_delay, interval, reply_to },
            ) => {
                if let Some(ps) = &s.peer_sharing_initiator {
                    eff.send(ps, PeerSharingMessage::Start { amount, initial_delay, interval, reply_to }).await;
                }
                State::Established(s)
            }
            (State::Established(s), ConnectionMessage::NewTip(tip, trace_context)) => {
                if let Some(cs) = &s.chainsync_responder {
                    eff.send(cs, chainsync::ResponderMessage::NewTip(tip, trace_context)).await;
                }
                State::Established(s)
            }
            (State::Established(mut s), ConnectionMessage::SetLocalUse(desired)) => {
                s.desired_use = desired;
                // Starting extra groups on a duplex bearer is a later PR. Stopping is MsgDone.
                // This commit only records the intent so later commits can converge actual_use.
                State::Established(s)
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
            (state @ (State::Initial | State::Handshake { .. }), msg @ ConnectionMessage::SetLocalUse(_)) => {
                eff.schedule_after(msg, params.config.reconnect_delay).await;
                state
            }
            x => unimplemented!("{x:?}"),
        };
        Connection { params, state }
    }
    .instrument(debug_span!(
        protocols::connection::message::PROCESS,
        message_type,
        conn_id = conn_id.as_u64(),
        peer,
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
        State::Established(s) if s.chainsync_initiator.is_some() => {
            notify_chainsync_terminated(params, eff).await;
        }
        State::Initial | State::Handshake { .. } | State::Established(_) => {}
    }
    eff.terminate().await
}

async fn notify_chainsync_terminated(params: &Params, eff: &Effects<ConnectionMessage>) {
    eff.send(
        &params.pipeline,
        ChainSyncInitiatorMsg {
            peer: params.peer,
            conn_id: params.conn_id,
            handler: StageRef::blackhole(),
            msg: InitiatorResult::Terminated,
        },
    )
    .await;
}

async fn do_initialize(Params { conn_id, role, magic, peer, .. }: &Params, eff: Effects<ConnectionMessage>) -> State {
    let peer = *peer;
    let muxer = eff.stage("mux", mux::stage).await;
    let muxer = eff.supervise(muxer, ConnectionMessage::ChildDied(ChildId::Mux));
    let muxer = eff.wire_up(muxer, mux::State::new(*conn_id, &[(PROTO_HANDSHAKE.erase(), 5760)], *role, peer)).await;

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
    let peer = *peer;
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
            peer,
            stage: eff.me(),
            conn_id: *conn_id,
            role: *role,
            full_duplex_capable,
            full_duplex,
            advertisable,
        },
    )
    .await;

    eff.send(&muxer, mux::MuxMessage::SetSduTimeout(mux::SDU_TIMEOUT_ESTABLISHED)).await;

    let keepalive = register_keepalive(
        *role,
        peer,
        *conn_id,
        muxer.clone(),
        &eff,
        ConnectionMessage::ChildDied(ChildId::KeepAlive),
    )
    .await;
    let tx_submission = register_tx_submission(
        *role,
        peer,
        muxer.clone(),
        &eff,
        TxOrigin::Remote(peer),
        mempool_stage.clone(),
        config.tx_submission_params,
        era_history.clone(),
        ConnectionMessage::ChildDied(ChildId::TxSubmission),
    )
    .await;

    let local_use = LocalUse::default_for_role(*role);
    let mut established = Established {
        desired_use: local_use,
        actual_use: local_use,
        version_number,
        version_data,
        muxer: muxer.clone(),
        handshake,
        keepalive,
        tx_submission,
        chainsync_initiator: None,
        blockfetch_initiator: None,
        peer_sharing_initiator: None,
        chainsync_responder: None,
        blockfetch_responder: None,
        peer_sharing_responder: None,
    };

    if *role == Role::Initiator {
        established.chainsync_initiator = Some(
            register_chainsync_initiator(
                &muxer,
                peer,
                *conn_id,
                pipeline_ref,
                &eff,
                ConnectionMessage::ChildDied(ChildId::ChainSync),
            )
            .await,
        );
        established.blockfetch_initiator = Some(if let Some(n) = config.blockfetch_pipeline_n {
            register_blockfetch_initiator_pipelined(
                &muxer,
                peer,
                *conn_id,
                n,
                &eff,
                ConnectionMessage::ChildDied(ChildId::BlockFetch),
            )
            .await
        } else {
            register_blockfetch_initiator(
                &muxer,
                peer,
                *conn_id,
                &eff,
                ConnectionMessage::ChildDied(ChildId::BlockFetch),
            )
            .await
        });
        established.peer_sharing_initiator = Some(
            register_peer_sharing_initiator(
                &muxer,
                peer,
                *conn_id,
                &eff,
                ConnectionMessage::ChildDied(ChildId::PeerSharing),
            )
            .await,
        );
    } else {
        let store = Store::new(eff.clone());
        let upstream = store.get_best_chain_tip().await;
        established.chainsync_responder = Some(
            register_chainsync_responder(
                &muxer,
                upstream,
                peer,
                *conn_id,
                &eff,
                ConnectionMessage::ChildDied(ChildId::ChainSync),
            )
            .await,
        );
        established.blockfetch_responder =
            Some(register_blockfetch_responder(&muxer, &eff, ConnectionMessage::ChildDied(ChildId::BlockFetch)).await);
        established.peer_sharing_responder = Some(
            register_peer_sharing_responder(
                &muxer,
                peer,
                manager.clone(),
                &eff,
                ConnectionMessage::ChildDied(ChildId::PeerSharing),
            )
            .await,
        );
    }

    State::Established(established)
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
    use amaru_pure_stage::{
        Effect, StageGraph,
        simulation::{Run, SimulationBuilder},
    };
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

        running.run(Run::skip_wakeups()).assert_breakpoint("schedule");

        let reconnect_delay = ManagerConfig::default().reconnect_delay;
        {
            let hit = running.breakpoint_effect();
            let Effect::Schedule { id, .. } = hit.effect() else {
                panic!("Expected Schedule effect, got {:?}", hit.effect());
            };
            let delay = id.time().checked_since(start_time).unwrap();
            assert!(delay >= reconnect_delay);
        }

        running.clear_breakpoint("schedule");
        running.run(Run::default()).assert_sleeping();

        // Verify state remains the same
        let state = running.get_state(&connection_stage).unwrap();
        assert_eq!(state.state, connection_state);
    }

    // HELPERS

    fn test_connection(state: State) -> Connection {
        Connection {
            params: Params {
                peer: Peer::for_test(3009),
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
