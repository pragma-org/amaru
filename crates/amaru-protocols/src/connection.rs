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

use std::{collections::BTreeSet, fmt, sync::Arc};

use amaru_kernel::{EraHistory, NetworkMagic, Peer, Point};
use amaru_observability::{Instrument, TraceContext, debug_span, error, info};
use amaru_ouroboros::{ConnectionId, MempoolMsg, TxOrigin};
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, TrySend, Void, register_data_deserializer};

use crate::{
    blockfetch::{self, BlockFetchMessage, Blocks, register_blockfetch_initiator, register_blockfetch_responder},
    chainsync::{
        self, ChainSyncInitiatorMsg, InitiatorResult, register_chainsync_initiator, register_chainsync_responder,
    },
    handshake,
    keepalive::{self, register_keepalive},
    manager::{ManagerConfig, ManagerMessage},
    mux::{self, MuxMessage},
    peer_sharing::{PeerSharingMessage, ShareResult, register_peer_sharing_initiator, register_peer_sharing_responder},
    protocol::{
        Erased, Inputs, PROTO_HANDSHAKE, PROTO_N2N_BLOCK_FETCH, PROTO_N2N_CHAIN_SYNC, PROTO_N2N_KEEP_ALIVE,
        PROTO_N2N_PEER_SHARE, PROTO_N2N_TX_SUB, ProtocolId, Role, ingress_limit,
    },
    protocol_messages::{
        handshake::HandshakeResult, version_data::VersionData, version_number::VersionNumber,
        version_table::VersionTable,
    },
    store_effects::Store,
    tx_submission::{self, register_tx_submission},
};

const STOP_TIMEOUT_SLOT: u64 = 1;

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
    fn default_for_role(role: Role) -> Self {
        match role {
            Role::Initiator => Self::Diffusion,
            Role::Responder => Self::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Maintenance => "maintenance",
            Self::Diffusion => "diffusion",
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
    duplex: bool,
    version_number: VersionNumber,
    version_data: VersionData,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    keepalive_initiator: Option<StageRef<keepalive::InitiatorMessage>>,
    tx_submission_initiator: Option<StageRef<tx_submission::InitiatorLocalIn>>,
    chainsync_initiator: Option<StageRef<chainsync::InitiatorMessage>>,
    blockfetch_initiator: Option<StageRef<blockfetch::BlockFetchMessage>>,
    peer_sharing_initiator: Option<StageRef<PeerSharingMessage>>,
    chainsync_responder: Option<StageRef<chainsync::ResponderMessage>>,
    blockfetch_responder: Option<StageRef<Void>>,
    peer_sharing_responder: Option<StageRef<crate::peer_sharing::ResponderMessage>>,
    stopping: BTreeSet<ChildId>,
    /// Latest tip the chainsync responder did not accept.
    ///
    /// Flushed with `try_send` at the start of the next transition. A newer tip replaces the
    /// stored one. `Queued` or `Gone` drops it; `Full` keeps it for the transition after that.
    pending_tip: Option<(Point, TraceContext)>,
    /// One `PeerSharingMessage::Start` the peer-sharing child did not accept.
    ///
    /// Flushed with `try_send` at the start of the next transition. A newer Start replaces the
    /// stored one. `Queued` or `Gone` drops it; `Full` keeps it. Cleared when that child is
    /// stopped or dies.
    pending_share: Option<PeerSharingMessage>,
}

/// Identity of a supervised child stage of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ChildId {
    Mux,
    Handshake,
    KeepAlive,
    TxSubmission,
    ChainSync,
    BlockFetch,
    PeerSharing,
    /// Any eager responder instance. Death is always unexpected (reset in place, do not stop).
    Responder,
}

impl fmt::Display for ChildId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
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
    /// Record the desired local use for a live connection.
    ///
    /// The connection stage does not currently reconcile `actual_use`.
    SetLocalUse(LocalUse),
    /// Last-to-finish group stop exceeded its bound.
    StopTimeout,
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
            ConnectionMessage::StopTimeout => "StopTimeout",
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
    let (local_use, duplex, stopping) = match &state {
        State::Established(s) => (s.actual_use.as_str(), s.duplex, s.stopping.len() as u64),
        State::Initial | State::Handshake { .. } => ("negotiating", false, 0),
    };

    async move {
        let state = flush_pending_tip(state, &eff).await;
        let state = flush_pending_share(state, &eff).await;
        let state = match (state, msg) {
            (state, ConnectionMessage::Disconnect) => {
                return teardown(state, &params, &eff).await;
            }
            (State::Established(s), ConnectionMessage::ChildDied(child)) if s.stopping.contains(&child) => {
                info!(
                    protocols::connection::CHILD_STOPPED,
                    peer = &params.peer,
                    conn_id = conn_id.as_u64(),
                    child = child.to_string()
                );
                State::Established(on_expected_stop(s, child, &params, &eff).await)
            }
            (state, ConnectionMessage::ChildDied(child)) => {
                info!(
                    protocols::connection::CHILD_DIED,
                    peer = &params.peer,
                    conn_id = conn_id.as_u64(),
                    child = child.to_string()
                );
                return teardown(clear_pending_share(state, child), &params, &eff).await;
            }
            (State::Established(s), ConnectionMessage::StopTimeout) => {
                if s.stopping.is_empty() {
                    State::Established(s)
                } else {
                    return teardown(State::Established(s), &params, &eff).await;
                }
            }
            (State::Initial, ConnectionMessage::Initialize) => do_initialize(&params, eff).await,
            (State::Handshake { muxer, handshake }, ConnectionMessage::Handshake(handshake_result)) => {
                do_handshake(&params, muxer, handshake, handshake_result, eff).await
            }
            (State::Established(s), ConnectionMessage::FetchBlocks { from, through, id, cr }) => {
                if !s.stopping.contains(&ChildId::BlockFetch)
                    && let Some(blockfetch) = &s.blockfetch_initiator
                    && eff
                        .try_send(blockfetch, BlockFetchMessage::RequestRange { from, through, id, cr: cr.clone() })
                        .await
                        == TrySend::Queued
                {
                    // Only a handler that admitted the range was asked. `Full` and a missing
                    // initiator say nothing: no `PeersAsked`, and no `NoBlocks`.
                    eff.send(&cr, Blocks::PeersAsked(id, vec![params.peer])).await;
                }
                State::Established(s)
            }
            (
                State::Established(mut s),
                ConnectionMessage::RequestSharePeers { amount, initial_delay, interval, reply_to },
            ) => {
                if !s.stopping.contains(&ChildId::PeerSharing)
                    && let Some(ps) = s.peer_sharing_initiator.clone()
                {
                    let start = PeerSharingMessage::Start { amount, initial_delay, interval, reply_to };
                    match eff.try_send(&ps, start.clone()).await {
                        TrySend::Full => s.pending_share = Some(start),
                        TrySend::Queued | TrySend::Gone => s.pending_share = None,
                    }
                } else {
                    s.pending_share = None;
                }
                State::Established(s)
            }
            (State::Established(mut s), ConnectionMessage::NewTip(tip, trace_context)) => {
                if let Some(cs) = s.chainsync_responder.clone() {
                    match eff.try_send(&cs, chainsync::ResponderMessage::NewTip(tip, trace_context.clone())).await {
                        TrySend::Full => s.pending_tip = Some((tip, trace_context)),
                        TrySend::Queued | TrySend::Gone => s.pending_tip = None,
                    }
                } else {
                    s.pending_tip = None;
                }
                State::Established(s)
            }
            (State::Established(mut s), ConnectionMessage::SetLocalUse(desired)) => {
                // Record only; `actual_use` is not reconciled here.
                s.desired_use = desired;
                State::Established(converge_use(s, &params, &eff).await)
            }
            (state @ (State::Initial | State::Handshake { .. }), msg @ ConnectionMessage::FetchBlocks { .. }) => {
                // The peer might still be connecting. Reschedule until the attempt finishes;
                // if it never does, the caller times out. The delay is the reconnect delay
                // (2s by default), shorter than the 5s call timeout. The connect attempt
                // itself fails after 2s.
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
            (state @ (State::Initial | State::Handshake { .. }), ConnectionMessage::StopTimeout) => state,
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
        local_use,
        duplex,
        stopping,
    ))
    .await
}

/// Offer a stored tip to the chainsync responder before this transition handles its message.
///
/// `Queued` and `Gone` drop the stored tip. `Full` keeps it for the following transition.
async fn flush_pending_tip(state: State, eff: &Effects<ConnectionMessage>) -> State {
    let State::Established(mut established) = state else {
        return state;
    };
    let Some((tip, trace_context)) = established.pending_tip.clone() else {
        return State::Established(established);
    };
    let Some(responder) = established.chainsync_responder.clone() else {
        established.pending_tip = None;
        return State::Established(established);
    };
    match eff.try_send(&responder, chainsync::ResponderMessage::NewTip(tip, trace_context)).await {
        TrySend::Queued | TrySend::Gone => established.pending_tip = None,
        TrySend::Full => {}
    }
    State::Established(established)
}

/// Offer a stored peer-sharing `Start` before this transition handles its message.
///
/// `Queued` and `Gone` drop the stored start. `Full` keeps it for the following transition.
/// A missing initiator drops it: there is no child left to retry.
async fn flush_pending_share(state: State, eff: &Effects<ConnectionMessage>) -> State {
    let State::Established(mut established) = state else {
        return state;
    };
    let Some(start) = established.pending_share.clone() else {
        return State::Established(established);
    };
    let Some(initiator) = established.peer_sharing_initiator.clone() else {
        established.pending_share = None;
        return State::Established(established);
    };
    match eff.try_send(&initiator, start).await {
        TrySend::Queued | TrySend::Gone => established.pending_share = None,
        TrySend::Full => {}
    }
    State::Established(established)
}

/// A dead peer-sharing child will not accept the stored `Start`.
fn clear_pending_share(state: State, child: ChildId) -> State {
    if child != ChildId::PeerSharing {
        return state;
    }
    let State::Established(mut established) = state else {
        return state;
    };
    established.pending_share = None;
    State::Established(established)
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

/// Protocols whose bytes the mux holds until `Register` installs the handler.
///
/// A responder always serves. An initiator serves when it advertises full duplex, which is
/// the case for every connection this node opens. Peer sharing is included when this side
/// advertises it. Each direction asks [`ingress_limit`] for its own protocol id (the responder
/// bit included). Any protocol id absent from this list still fails the connection.
fn early_mini_protocol_buffers(advertisable: bool) -> Vec<(ProtocolId<Erased>, usize)> {
    let mut both =
        vec![PROTO_HANDSHAKE, PROTO_N2N_CHAIN_SYNC, PROTO_N2N_BLOCK_FETCH, PROTO_N2N_TX_SUB, PROTO_N2N_KEEP_ALIVE];
    if advertisable {
        both.push(PROTO_N2N_PEER_SHARE);
    }
    let mut buffers = Vec::with_capacity(both.len() * 2);
    for id in both {
        buffers.push((id.erase(), ingress_limit(id)));
        buffers.push((id.responder().erase(), ingress_limit(id.responder())));
    }
    buffers
}

async fn do_initialize(
    Params { conn_id, role, magic, peer, config, .. }: &Params,
    eff: Effects<ConnectionMessage>,
) -> State {
    let peer = *peer;
    // Same flags as the version table below. A responder always serves; an initiator serves
    // when it advertises full duplex. Those are the protocols the mux may hold before `Register`.
    let initiator_only = false;
    let advertisable = true;
    let muxer = eff.stage_with_mailbox_size("mux", mux::stage, mux::MUX_MAILBOX_SIZE).await;
    let muxer = eff.supervise(muxer, ConnectionMessage::ChildDied(ChildId::Mux));
    let early = early_mini_protocol_buffers(advertisable);
    let muxer = eff.wire_up(muxer, mux::State::new(*conn_id, &early, *role, peer)).await;

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
                    VersionTable::v11_through(config.max_n2n_version, *magic, initiator_only, advertisable),
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
                    VersionTable::v11_through(config.max_n2n_version, *magic, initiator_only, advertisable),
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
    eff.send(
        &muxer,
        MuxMessage::Register { protocol, frame: mux::Frame::OneCborItem, handler, max_buffer: ingress_limit(protocol) },
    )
    .await;

    State::Handshake { muxer, handshake }
}

async fn do_handshake(
    params: &Params,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    handshake_result: HandshakeResult,
    eff: Effects<ConnectionMessage>,
) -> State {
    let Params { role, peer, conn_id, manager, .. } = params;
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
    let full_duplex = full_duplex_capable;
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

    let local_use = LocalUse::default_for_role(*role);
    let run_initiators = *role == Role::Initiator || full_duplex;
    let run_responders = *role == Role::Responder || full_duplex;
    let mut established = Established {
        desired_use: local_use,
        actual_use: LocalUse::None,
        duplex: full_duplex,
        version_number,
        version_data,
        muxer: muxer.clone(),
        handshake,
        keepalive_initiator: None,
        tx_submission_initiator: None,
        chainsync_initiator: None,
        blockfetch_initiator: None,
        peer_sharing_initiator: None,
        chainsync_responder: None,
        blockfetch_responder: None,
        peer_sharing_responder: None,
        stopping: BTreeSet::new(),
        pending_tip: None,
        pending_share: None,
    };

    if run_responders {
        established = register_responders(established, params, &eff).await;
    }
    if run_initiators && local_use > LocalUse::None {
        established = start_initiators(established, params, &eff).await;
    } else {
        established.actual_use = local_use;
        notify_local_use(&established, params, &eff).await;
    }
    State::Established(established)
}

async fn register_responders(mut s: Established, params: &Params, eff: &Effects<ConnectionMessage>) -> Established {
    let Params { peer, conn_id, manager, era_history, mempool_stage, config, .. } = params;
    let died = ConnectionMessage::ChildDied(ChildId::Responder);
    let _ = register_keepalive(Role::Responder, *peer, *conn_id, s.muxer.clone(), eff, died).await;
    let _ = register_tx_submission(
        Role::Responder,
        *peer,
        s.muxer.clone(),
        eff,
        TxOrigin::Remote(*peer),
        mempool_stage.clone(),
        config.tx_submission_params,
        era_history.clone(),
        ConnectionMessage::ChildDied(ChildId::Responder),
    )
    .await;
    let store = Store::new(eff.clone());
    let upstream = store.get_best_chain_tip().await;
    s.chainsync_responder = Some(
        register_chainsync_responder(
            &s.muxer,
            upstream,
            *peer,
            *conn_id,
            eff,
            ConnectionMessage::ChildDied(ChildId::Responder),
        )
        .await,
    );
    s.blockfetch_responder = Some(
        register_blockfetch_responder(&s.muxer, *peer, eff, ConnectionMessage::ChildDied(ChildId::Responder)).await,
    );
    if s.version_data.is_advertisable() {
        s.peer_sharing_responder = Some(
            register_peer_sharing_responder(
                &s.muxer,
                *peer,
                manager.clone(),
                eff,
                ConnectionMessage::ChildDied(ChildId::Responder),
            )
            .await,
        );
    }
    s
}

async fn converge_use(mut s: Established, params: &Params, eff: &Effects<ConnectionMessage>) -> Established {
    if !s.stopping.is_empty() {
        return s;
    }
    if s.desired_use < s.actual_use {
        begin_stop(s, params, eff).await
    } else if s.desired_use > s.actual_use && (params.role == Role::Initiator || s.duplex) {
        start_initiators(s, params, eff).await
    } else if s.desired_use != s.actual_use {
        s.actual_use = s.desired_use;
        notify_local_use(&s, params, eff).await;
        s
    } else {
        s
    }
}

async fn begin_stop(mut s: Established, params: &Params, eff: &Effects<ConnectionMessage>) -> Established {
    let drop_diffusion = s.actual_use >= LocalUse::Diffusion && s.desired_use < LocalUse::Diffusion;
    let drop_maintenance = s.actual_use >= LocalUse::Maintenance && s.desired_use < LocalUse::Maintenance;

    if drop_diffusion {
        if let Some(cs) = &s.chainsync_initiator {
            s.stopping.insert(ChildId::ChainSync);
            let _ = eff.try_send(cs, chainsync::InitiatorMessage::Done).await;
        }
        if let Some(bf) = &s.blockfetch_initiator {
            s.stopping.insert(ChildId::BlockFetch);
            let _ = eff.try_send(bf, BlockFetchMessage::Close).await;
        }
        if let Some(tx) = &s.tx_submission_initiator {
            s.stopping.insert(ChildId::TxSubmission);
            let _ = eff.try_send(tx, tx_submission::InitiatorLocalIn::Close).await;
        }
    }
    if drop_maintenance {
        if let Some(ka) = &s.keepalive_initiator {
            s.stopping.insert(ChildId::KeepAlive);
            let _ = eff.try_send(ka, keepalive::InitiatorMessage::Close).await;
        }
        if let Some(ps) = &s.peer_sharing_initiator {
            s.stopping.insert(ChildId::PeerSharing);
            s.pending_share = None;
            let _ = eff.try_send(ps, PeerSharingMessage::Close).await;
        }
    }

    if s.stopping.is_empty() {
        s.actual_use = s.desired_use;
        notify_local_use(&s, params, eff).await;
        return s;
    }

    let timeout =
        if drop_diffusion { params.config.diffusion_stop_timeout } else { params.config.maintenance_stop_timeout };
    eff.set_timeout_at(STOP_TIMEOUT_SLOT, timeout, ConnectionMessage::StopTimeout).await;
    s
}

async fn on_expected_stop(
    mut s: Established,
    child: ChildId,
    params: &Params,
    eff: &Effects<ConnectionMessage>,
) -> Established {
    s.stopping.remove(&child);
    match child {
        ChildId::ChainSync => {
            s.chainsync_initiator = None;
            mux::install_done_trap(
                &s.muxer,
                PROTO_N2N_CHAIN_SYNC.erase(),
                params.peer,
                eff,
                ConnectionMessage::ChildDied(child),
            )
            .await;
        }
        ChildId::BlockFetch => {
            s.blockfetch_initiator = None;
            mux::install_done_trap(
                &s.muxer,
                PROTO_N2N_BLOCK_FETCH.erase(),
                params.peer,
                eff,
                ConnectionMessage::ChildDied(child),
            )
            .await;
        }
        ChildId::TxSubmission => {
            s.tx_submission_initiator = None;
            mux::install_done_trap(
                &s.muxer,
                PROTO_N2N_TX_SUB.erase(),
                params.peer,
                eff,
                ConnectionMessage::ChildDied(child),
            )
            .await;
        }
        ChildId::KeepAlive => {
            s.keepalive_initiator = None;
            mux::install_done_trap(
                &s.muxer,
                PROTO_N2N_KEEP_ALIVE.erase(),
                params.peer,
                eff,
                ConnectionMessage::ChildDied(child),
            )
            .await;
        }
        ChildId::PeerSharing => {
            s.peer_sharing_initiator = None;
            s.pending_share = None;
            mux::install_done_trap(
                &s.muxer,
                PROTO_N2N_PEER_SHARE.erase(),
                params.peer,
                eff,
                ConnectionMessage::ChildDied(child),
            )
            .await;
        }
        ChildId::Mux | ChildId::Handshake | ChildId::Responder => {}
    }

    if s.stopping.is_empty() {
        eff.clear_timeout_at(STOP_TIMEOUT_SLOT).await;
        s.actual_use = s.desired_use;
        notify_local_use(&s, params, eff).await;
        if s.desired_use > LocalUse::None && (params.role == Role::Initiator || s.duplex) {
            return start_initiators(s, params, eff).await;
        }
    }
    s
}

async fn start_initiators(mut s: Established, params: &Params, eff: &Effects<ConnectionMessage>) -> Established {
    let Params { peer, conn_id, config, pipeline, mempool_stage, era_history, .. } = params;
    if s.desired_use >= LocalUse::Maintenance {
        if s.keepalive_initiator.is_none() {
            s.keepalive_initiator = register_keepalive(
                Role::Initiator,
                *peer,
                *conn_id,
                s.muxer.clone(),
                eff,
                ConnectionMessage::ChildDied(ChildId::KeepAlive),
            )
            .await;
        }
        if s.peer_sharing_initiator.is_none() {
            s.peer_sharing_initiator = Some(
                register_peer_sharing_initiator(
                    &s.muxer,
                    *peer,
                    *conn_id,
                    eff,
                    ConnectionMessage::ChildDied(ChildId::PeerSharing),
                )
                .await,
            );
        }
    }
    if s.desired_use == LocalUse::Diffusion {
        if s.chainsync_initiator.is_none() {
            s.chainsync_initiator = Some(
                register_chainsync_initiator(
                    &s.muxer,
                    *peer,
                    *conn_id,
                    pipeline.clone(),
                    eff,
                    ConnectionMessage::ChildDied(ChildId::ChainSync),
                )
                .await,
            );
        }
        if s.blockfetch_initiator.is_none() {
            s.blockfetch_initiator = Some(
                register_blockfetch_initiator(
                    &s.muxer,
                    *peer,
                    config.blockfetch_pipeline_n,
                    eff,
                    ConnectionMessage::ChildDied(ChildId::BlockFetch),
                )
                .await,
            );
        }
        if s.tx_submission_initiator.is_none() {
            s.tx_submission_initiator = register_tx_submission(
                Role::Initiator,
                *peer,
                s.muxer.clone(),
                eff,
                TxOrigin::Remote(*peer),
                mempool_stage.clone(),
                config.tx_submission_params,
                era_history.clone(),
                ConnectionMessage::ChildDied(ChildId::TxSubmission),
            )
            .await;
        }
    }
    s.actual_use = s.desired_use;
    notify_local_use(&s, params, eff).await;
    s
}

async fn notify_local_use(s: &Established, params: &Params, eff: &Effects<ConnectionMessage>) {
    eff.send(
        &params.manager,
        ManagerMessage::LocalUseApplied { peer: params.peer, conn_id: params.conn_id, local_use: s.actual_use },
    )
    .await;
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
    use std::time::Duration;

    use amaru_kernel::{BlockHeight, HeaderHash, PREPROD_ERA_HISTORY, Slot};
    use amaru_pure_stage::{
        DEFAULT_MAILBOX_SIZE, Effect, Name, SendData, StageGraph, StageResponse, TraceMatch,
        simulation::{Run, SimulationBuilder, SimulationRunning},
        stage_ref::StageStateRef,
        trace_buffer::{TraceBuffer, TraceEntry},
        trace_match::{
            assert_trace_contains, assert_trace_match_filter, tm_resume_try_send, tm_send, tm_state_match,
            tm_try_send_match,
        },
    };
    use tokio::runtime::Runtime;

    use super::*;
    use crate::protocol_messages::version_data::PeerSharing;

    /// Limits installed for each mini-protocol id, initiator and responder.
    ///
    /// These are the mux ingress sizes from ouroboros-network `maximumIngressQueue`
    /// (network-spec table 3.15; handshake is the 4×1440 transmission unit). The
    /// assertion goes through [`early_mini_protocol_buffers`], which is what the mux
    /// is constructed with — not a second copy of that table.
    #[test]
    fn ingress_limit_for_each_protocol_and_role() {
        let buffers = early_mini_protocol_buffers(true);
        let installed = |id: ProtocolId<Erased>| -> usize {
            buffers.iter().find(|(proto, _)| *proto == id).map(|(_, n)| *n).expect("protocol is buffered")
        };
        for (id, expected) in [
            (PROTO_HANDSHAKE, 5_760usize),
            (PROTO_N2N_CHAIN_SYNC, 462_000),
            (PROTO_N2N_BLOCK_FETCH, 23_068_694),
            (PROTO_N2N_TX_SUB, 721_424),
            (PROTO_N2N_KEEP_ALIVE, 1_408),
            (PROTO_N2N_PEER_SHARE, 5_760),
        ] {
            assert_eq!(ingress_limit(id), expected, "initiator {id}");
            assert_eq!(ingress_limit(id.responder()), expected, "responder {}", id.responder());
            assert_eq!(installed(id.erase()), ingress_limit(id), "early initiator {id}");
            assert_eq!(installed(id.responder().erase()), ingress_limit(id.responder()), "early responder {id}");
        }
    }

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

    #[test]
    fn mux_is_created_with_the_burst_mailbox() {
        let _guards = trace_guards();
        let mut network = SimulationBuilder::default();
        let connection = network.stage("connection", stage);
        let connection = network.wire_up(connection, test_connection(State::Initial));
        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.breakpoint(
            "mux-wire",
            |eff| matches!(eff, Effect::WireStage { name, .. } if name.as_str().starts_with("mux")),
        );
        running.enqueue_msg(&connection, [ConnectionMessage::Initialize]);
        running.run(Run::default()).assert_breakpoint("mux-wire");
        let hit = running.breakpoint_effect();
        let Effect::WireStage { mailbox_size, .. } = hit.effect() else {
            panic!("expected the mux to be wired");
        };
        assert_eq!(mux::MUX_MAILBOX_SIZE, 24);
        assert_eq!(*mailbox_size, 24);
        assert_eq!(*mailbox_size, mux::MUX_MAILBOX_SIZE);
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

    fn tip(slot: u64, byte: u8) -> Point {
        Point::Specific(Slot::from(slot), HeaderHash::new([byte; 32]), BlockHeight::from(slot))
    }

    fn established(
        blockfetch: Option<StageRef<BlockFetchMessage>>,
        chainsync: Option<StageRef<chainsync::ResponderMessage>>,
    ) -> Connection {
        test_connection(State::Established(Established {
            desired_use: LocalUse::Diffusion,
            actual_use: LocalUse::Diffusion,
            duplex: true,
            version_number: VersionNumber::CURRENT,
            version_data: VersionData::new(NetworkMagic::PREPROD, false, PeerSharing::Disabled, false),
            muxer: StageRef::blackhole(),
            handshake: StageRef::blackhole(),
            keepalive_initiator: None,
            tx_submission_initiator: None,
            chainsync_initiator: None,
            blockfetch_initiator: blockfetch,
            peer_sharing_initiator: None,
            chainsync_responder: chainsync,
            blockfetch_responder: None,
            peer_sharing_responder: None,
            stopping: BTreeSet::new(),
            pending_tip: None,
            pending_share: None,
        }))
    }

    fn established_sharing(
        peer_sharing: Option<StageRef<PeerSharingMessage>>,
        pending: Option<PeerSharingMessage>,
        stopping: BTreeSet<ChildId>,
    ) -> Connection {
        let mut connection = established(None, None);
        let State::Established(established) = &mut connection.state else {
            unreachable!("established() builds Established");
        };
        established.peer_sharing_initiator = peer_sharing;
        established.pending_share = pending;
        established.stopping = stopping;
        connection
    }

    fn connection_input<'a>(
        stage: &'a str,
        predicate: impl Fn(&ConnectionMessage) -> bool + Send + 'a,
    ) -> TraceMatch<'a> {
        let stage = stage.to_string();
        let description = format!("Input at {stage}");
        TraceMatch::Property(
            Box::new(move |src| {
                let Some(TraceEntry::Input { stage: got, input }) = src.entry() else {
                    return false;
                };
                got.as_str() == stage && input.cast_ref::<ConnectionMessage>().is_ok_and(&predicate)
            }),
            description,
        )
    }

    fn traced() -> SimulationBuilder {
        SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1_000_000))
    }

    fn trace_guards() -> amaru_pure_stage::DeserializerGuards {
        let mut guards = crate::deserializers::register_deserializers();
        guards.push(register_data_deserializer::<Inputs<BlockFetchMessage>>().boxed());
        guards.push(register_data_deserializer::<Inputs<chainsync::ResponderMessage>>().boxed());
        guards.push(register_data_deserializer::<Inputs<PeerSharingMessage>>().boxed());
        guards
    }

    fn drop_other_stages(keep: &str) -> TraceMatch<'static> {
        let keep = keep.to_string();
        let description = format!("stage other than {keep}");
        TraceMatch::Property(
            Box::new(move |src| {
                src.entry().and_then(|entry| entry.at_stage()).is_some_and(|stage| stage.as_str() != keep)
            }),
            description,
        )
    }

    /// Drops resumes other than [`StageResponse::TrySend`]. The admission result is that resume.
    fn drop_resume_except_try_send() -> TraceMatch<'static> {
        TraceMatch::Property(
            Box::new(|src| match src.entry() {
                Some(TraceEntry::Resume { response: StageResponse::TrySend(_), .. }) => false,
                Some(TraceEntry::Resume { .. }) => true,
                _ => false,
            }),
            "Resume other than TrySend".to_string(),
        )
    }

    async fn hold_blockfetch(_state: (), _msg: Inputs<BlockFetchMessage>, eff: Effects<Inputs<BlockFetchMessage>>) {
        eff.wait(Duration::from_secs(3600)).await;
    }

    async fn hold_chainsync(
        _state: (),
        _msg: Inputs<chainsync::ResponderMessage>,
        eff: Effects<Inputs<chainsync::ResponderMessage>>,
    ) {
        eff.wait(Duration::from_secs(3600)).await;
    }

    async fn collect_blocks(mut seen: Vec<Blocks>, msg: Blocks, _eff: Effects<Blocks>) -> Vec<Blocks> {
        seen.push(msg);
        seen
    }

    fn fill_to_capacity<Msg: SendData>(
        running: &mut SimulationRunning,
        stage: &impl AsRef<StageRef<Msg>>,
        msg: impl Fn() -> Msg,
    ) {
        running.enqueue_msg(stage, [msg()]);
        running.run(Run::default()).assert_sleeping();
        for _ in 0..DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(stage, [msg()]);
        }
        assert_eq!(running.mailbox_len(stage), DEFAULT_MAILBOX_SIZE);
    }

    fn pending_share_of(connection: &Connection) -> Option<PeerSharingMessage> {
        let State::Established(established) = &connection.state else {
            panic!("connection left Established");
        };
        established.pending_share.clone()
    }

    fn share_start(amount: u8) -> PeerSharingMessage {
        PeerSharingMessage::Start {
            amount,
            initial_delay: Duration::from_secs(1),
            interval: Duration::from_secs(60),
            reply_to: StageRef::blackhole(),
        }
    }

    fn share_request(amount: u8) -> ConnectionMessage {
        ConnectionMessage::RequestSharePeers {
            amount,
            initial_delay: Duration::from_secs(1),
            interval: Duration::from_secs(60),
            reply_to: StageRef::blackhole(),
        }
    }

    fn is_local_start(amount: u8) -> impl Fn(&Inputs<PeerSharingMessage>) -> bool {
        move |msg| matches!(msg, Inputs::Local(PeerSharingMessage::Start { amount: got, .. }) if *got == amount)
    }

    fn is_start(amount: u8) -> impl Fn(&PeerSharingMessage) -> bool {
        move |msg| matches!(msg, PeerSharingMessage::Start { amount: got, .. } if *got == amount)
    }

    async fn hold_share(_state: (), _msg: Inputs<PeerSharingMessage>, eff: Effects<Inputs<PeerSharingMessage>>) {
        eff.wait(Duration::from_secs(3600)).await;
    }

    fn pending_point(connection: &Connection) -> Option<Point> {
        let State::Established(established) = &connection.state else {
            panic!("connection left Established");
        };
        established.pending_tip.as_ref().map(|(point, _)| *point)
    }

    fn is_local_tip(point: Point) -> impl Fn(&Inputs<chainsync::ResponderMessage>) -> bool {
        move |msg| matches!(msg, Inputs::Local(chainsync::ResponderMessage::NewTip(got, _)) if *got == point)
    }

    #[test]
    fn nonblocking_fetch_full_child_emits_nothing() {
        let _guards = trace_guards();
        let mut network = traced();
        let blockfetch = network.stage("blockfetch", hold_blockfetch);
        let blockfetch_sender = blockfetch.sender();
        let blockfetch = network.wire_up(blockfetch, ());
        let asked = network.stage("asked", collect_blocks);
        let asked_sender = asked.sender();
        let asked = network.wire_up(asked, Vec::new());
        let connection = network.stage("connection", stage);
        let connection = network.wire_up(
            connection,
            established(Some(blockfetch_sender.contramap(Inputs::<BlockFetchMessage>::Local)), None),
        );

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        fill_to_capacity(&mut running, &blockfetch, || Inputs::Local(BlockFetchMessage::Close));
        running.trace_buffer().lock().clear();

        let msg =
            ConnectionMessage::FetchBlocks { from: Point::Origin, through: Point::Origin, id: 7, cr: asked_sender };
        running.enqueue_msg(&connection, [msg]);
        running.run(Run::default()).assert_sleeping();

        assert!(running.get_state(&connection).is_some(), "connection waited on a full block-fetch handler");
        assert!(running.get_state(&asked).unwrap().is_empty());
        assert_eq!(running.mailbox_len(&blockfetch), DEFAULT_MAILBOX_SIZE);

        let name = connection.name().as_str();
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name, |sent| matches!(sent, ConnectionMessage::FetchBlocks { id: 7, .. })),
                tm_try_send_match(name, "blockfetch", |sent: &Inputs<BlockFetchMessage>| {
                    matches!(sent, Inputs::Local(BlockFetchMessage::RequestRange { id: 7, .. }))
                }),
                tm_resume_try_send(name, TrySend::Full),
                tm_state_match(name, |state: &Connection| pending_point(state).is_none()),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name)],
        );
    }

    #[test]
    fn nonblocking_fetch_queued_child_reports_peers_asked() {
        let _guards = trace_guards();
        let mut network = traced();
        let blockfetch = network.stage("blockfetch", hold_blockfetch);
        let blockfetch_sender = blockfetch.sender();
        let blockfetch = network.wire_up(blockfetch, ());
        let asked = network.stage("asked", collect_blocks);
        let asked_sender = asked.sender();
        let asked = network.wire_up(asked, Vec::new());
        let connection = network.stage("connection", stage);
        let connection = network.wire_up(
            connection,
            established(Some(blockfetch_sender.contramap(Inputs::<BlockFetchMessage>::Local)), None),
        );
        let peer = Peer::for_test(3009);

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        running.trace_buffer().lock().clear();

        let msg =
            ConnectionMessage::FetchBlocks { from: Point::Origin, through: Point::Origin, id: 7, cr: asked_sender };
        running.enqueue_msg(&connection, [msg]);
        running.run(Run::default()).assert_sleeping();

        assert!(running.get_state(&connection).is_some());
        assert_eq!(running.get_state(&asked).unwrap().as_slice(), &[Blocks::PeersAsked(7, vec![peer])]);
        assert_eq!(running.mailbox_len(&blockfetch), 0, "the child took the one admitted request");

        let name = connection.name().as_str();
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name, |sent| matches!(sent, ConnectionMessage::FetchBlocks { id: 7, .. })),
                tm_try_send_match(name, "blockfetch", |sent: &Inputs<BlockFetchMessage>| {
                    matches!(sent, Inputs::Local(BlockFetchMessage::RequestRange { id: 7, .. }))
                }),
                tm_resume_try_send(name, TrySend::Queued),
                tm_send(name, "asked", Blocks::PeersAsked(7, vec![peer])),
                tm_state_match(name, |state: &Connection| pending_point(state).is_none()),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name)],
        );
    }

    #[test]
    fn nonblocking_new_tip_keeps_latest_and_flushes_once() {
        let _guards = trace_guards();
        let mut network = traced();
        let chainsync = network.stage("chainsync", hold_chainsync);
        let chainsync_sender = chainsync.sender();
        let chainsync = network.wire_up(chainsync, ());
        let connection = network.stage("connection", stage);
        let connection = network.wire_up(
            connection,
            established(None, Some(chainsync_sender.contramap(Inputs::<chainsync::ResponderMessage>::Local))),
        );

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        let parked = fill_and_remember_wakeup(&mut running, &chainsync);
        running.trace_buffer().lock().clear();

        let first = tip(1, 1);
        let second = tip(2, 2);
        let name = connection.name().clone();
        offer_tip(&mut running, &connection, &name, first);
        assert_eq!(pending_point(running.get_state(&connection).unwrap()), Some(first));

        running.trace_buffer().lock().clear();
        running.enqueue_msg(&connection, [ConnectionMessage::new_tip(second)]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(pending_point(running.get_state(&connection).unwrap()), Some(second));
        assert_trace_match_filter(
            &running,
            &[
                connection_input(
                    name.as_str(),
                    move |sent| matches!(sent, ConnectionMessage::NewTip(got, _) if *got == second),
                ),
                tm_try_send_match(name.as_str(), "chainsync", is_local_tip(first)),
                tm_resume_try_send(name.as_str(), TrySend::Full),
                tm_try_send_match(name.as_str(), "chainsync", is_local_tip(second)),
                tm_resume_try_send(name.as_str(), TrySend::Full),
                tm_state_match(name.as_str(), move |state: &Connection| pending_point(state) == Some(second)),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );

        running.run(Run::until(parked)).assert_sleeping();
        assert_eq!(running.mailbox_len(&chainsync), DEFAULT_MAILBOX_SIZE - 1);
        running.trace_buffer().lock().clear();

        running.enqueue_msg(&connection, [ConnectionMessage::StopTimeout]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(pending_point(running.get_state(&connection).unwrap()), None);
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name.as_str(), |sent| matches!(sent, ConnectionMessage::StopTimeout)),
                tm_try_send_match(name.as_str(), "chainsync", is_local_tip(second)),
                tm_resume_try_send(name.as_str(), TrySend::Queued),
                tm_state_match(name.as_str(), |state: &Connection| pending_point(state).is_none()),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );
    }

    fn fill_and_remember_wakeup(
        running: &mut SimulationRunning,
        stage: &StageStateRef<Inputs<chainsync::ResponderMessage>, ()>,
    ) -> amaru_pure_stage::Instant {
        running.enqueue_msg(
            stage,
            [Inputs::Local(chainsync::ResponderMessage::NewTip(Point::Origin, TraceContext::none()))],
        );
        let parked = running.run(Run::default()).assert_sleeping();
        for _ in 0..DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(
                stage,
                [Inputs::Local(chainsync::ResponderMessage::NewTip(Point::Origin, TraceContext::none()))],
            );
        }
        assert_eq!(running.mailbox_len(stage), DEFAULT_MAILBOX_SIZE);
        parked
    }

    fn offer_tip(
        running: &mut SimulationRunning,
        connection: &StageStateRef<ConnectionMessage, Connection>,
        name: &Name,
        point: Point,
    ) {
        running.enqueue_msg(connection, [ConnectionMessage::new_tip(point)]);
        running.run(Run::default()).assert_sleeping();
        assert_trace_match_filter(
            running,
            &[
                connection_input(
                    name.as_str(),
                    move |sent| matches!(sent, ConnectionMessage::NewTip(got, _) if *got == point),
                ),
                tm_try_send_match(name.as_str(), "chainsync", is_local_tip(point)),
                tm_resume_try_send(name.as_str(), TrySend::Full),
                tm_state_match(name.as_str(), move |state: &Connection| pending_point(state) == Some(point)),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );
    }

    fn fill_share(
        running: &mut SimulationRunning,
        stage: &StageStateRef<Inputs<PeerSharingMessage>, ()>,
    ) -> amaru_pure_stage::Instant {
        running.enqueue_msg(stage, [Inputs::Local(PeerSharingMessage::Tick)]);
        let parked = running.run(Run::default()).assert_sleeping();
        for _ in 0..DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(stage, [Inputs::Local(PeerSharingMessage::Tick)]);
        }
        assert_eq!(running.mailbox_len(stage), DEFAULT_MAILBOX_SIZE);
        parked
    }

    /// A full peer-sharing child keeps the one Start. A second Start replaces it. The next
    /// transition flushes that latest Start once.
    #[test]
    fn nonblocking_share_keeps_latest_start_and_flushes_once() {
        let _guards = trace_guards();
        let mut network = traced();
        let sharing = network.stage("sharing", hold_share);
        let sharing_sender = sharing.sender();
        let sharing = network.wire_up(sharing, ());
        let connection = network.stage("connection", stage);
        let connection = network.wire_up(
            connection,
            established_sharing(
                Some(sharing_sender.contramap(Inputs::<PeerSharingMessage>::Local)),
                None,
                BTreeSet::new(),
            ),
        );

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        let parked = fill_share(&mut running, &sharing);
        running.trace_buffer().lock().clear();

        let name = connection.name().clone();
        running.enqueue_msg(&connection, [share_request(1)]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(pending_share_of(running.get_state(&connection).unwrap()), Some(share_start(1)));
        assert_eq!(running.mailbox_len(&sharing), DEFAULT_MAILBOX_SIZE);
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name.as_str(), |sent| {
                    matches!(sent, ConnectionMessage::RequestSharePeers { amount: 1, .. })
                }),
                tm_try_send_match(name.as_str(), "sharing", is_local_start(1)),
                tm_resume_try_send(name.as_str(), TrySend::Full),
                tm_state_match(name.as_str(), |state: &Connection| pending_share_of(state) == Some(share_start(1))),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );

        running.enqueue_msg(&connection, [share_request(2)]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(pending_share_of(running.get_state(&connection).unwrap()), Some(share_start(2)));
        assert_eq!(
            running.mailbox_len(&sharing),
            DEFAULT_MAILBOX_SIZE,
            "a second Start must not be queued beside the first"
        );
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name.as_str(), |sent| {
                    matches!(sent, ConnectionMessage::RequestSharePeers { amount: 2, .. })
                }),
                tm_try_send_match(name.as_str(), "sharing", is_local_start(1)),
                tm_resume_try_send(name.as_str(), TrySend::Full),
                tm_try_send_match(name.as_str(), "sharing", is_local_start(2)),
                tm_resume_try_send(name.as_str(), TrySend::Full),
                tm_state_match(name.as_str(), |state: &Connection| pending_share_of(state) == Some(share_start(2))),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );

        running.run(Run::until(parked)).assert_sleeping();
        assert_eq!(running.mailbox_len(&sharing), DEFAULT_MAILBOX_SIZE - 1);
        running.trace_buffer().lock().clear();

        running.enqueue_msg(&connection, [ConnectionMessage::StopTimeout]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(pending_share_of(running.get_state(&connection).unwrap()), None);
        assert_eq!(running.mailbox_len(&sharing), DEFAULT_MAILBOX_SIZE);
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name.as_str(), |sent| matches!(sent, ConnectionMessage::StopTimeout)),
                tm_try_send_match(name.as_str(), "sharing", is_local_start(2)),
                tm_resume_try_send(name.as_str(), TrySend::Queued),
                tm_state_match(name.as_str(), |state: &Connection| pending_share_of(state).is_none()),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );
    }

    #[test]
    fn nonblocking_share_gone_clears_pending_start() {
        let _guards = trace_guards();
        let mut network = traced();
        let connection = network.stage("connection", stage);
        let gone = StageRef::<PeerSharingMessage>::named_for_tests("missing-share");
        let connection =
            network.wire_up(connection, established_sharing(Some(gone), Some(share_start(1)), BTreeSet::new()));

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        running.trace_buffer().lock().clear();

        let name = connection.name().clone();
        running.enqueue_msg(&connection, [ConnectionMessage::StopTimeout]);
        running.run(Run::default()).assert_idle();
        assert_eq!(pending_share_of(running.get_state(&connection).unwrap()), None);
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name.as_str(), |sent| matches!(sent, ConnectionMessage::StopTimeout)),
                tm_try_send_match(name.as_str(), "missing-share", is_start(1)),
                tm_resume_try_send(name.as_str(), TrySend::Gone),
                tm_state_match(name.as_str(), |state: &Connection| pending_share_of(state).is_none()),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );

        running.enqueue_msg(&connection, [share_request(2)]);
        running.run(Run::default()).assert_idle();
        assert_eq!(pending_share_of(running.get_state(&connection).unwrap()), None);
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name.as_str(), |sent| {
                    matches!(sent, ConnectionMessage::RequestSharePeers { amount: 2, .. })
                }),
                tm_try_send_match(name.as_str(), "missing-share", is_start(2)),
                tm_resume_try_send(name.as_str(), TrySend::Gone),
                tm_state_match(name.as_str(), |state: &Connection| pending_share_of(state).is_none()),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name.as_str())],
        );
    }

    #[test]
    fn nonblocking_share_child_died_clears_pending_start() {
        let _guards = trace_guards();
        let mut network = traced();
        let sharing = network.stage("sharing", hold_share);
        let sharing_sender = sharing.sender();
        let sharing = network.wire_up(sharing, ());
        let connection = network.stage("connection", stage);
        let mut initial = established_sharing(
            Some(sharing_sender.contramap(Inputs::<PeerSharingMessage>::Local)),
            Some(share_start(1)),
            BTreeSet::from([ChildId::PeerSharing]),
        );
        let State::Established(established) = &mut initial.state else {
            unreachable!("established_sharing builds Established");
        };
        // Desired use None so the expected stop does not start a replacement child.
        established.desired_use = LocalUse::None;
        established.actual_use = LocalUse::None;
        let connection = network.wire_up(connection, initial);

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        fill_share(&mut running, &sharing);
        running.trace_buffer().lock().clear();

        let name = connection.name().clone();
        running.enqueue_msg(&connection, [ConnectionMessage::ChildDied(ChildId::PeerSharing)]);
        running.run(Run::default()).assert_sleeping();

        let state = running.get_state(&connection).expect("expected child death finishes the transition");
        let State::Established(established) = &state.state else {
            panic!("connection left Established");
        };
        assert_eq!(established.pending_share, None);
        assert!(established.peer_sharing_initiator.is_none());
        assert!(established.stopping.is_empty());
        assert_eq!(running.mailbox_len(&sharing), DEFAULT_MAILBOX_SIZE);
        // `assert_trace_contains` drops resumes. The admission result is the resume.
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[
                connection_input(name.as_str(), |sent| {
                    matches!(sent, ConnectionMessage::ChildDied(ChildId::PeerSharing))
                }),
                tm_try_send_match(name.as_str(), "sharing", is_local_start(1)),
                tm_state_match(name.as_str(), |state: &Connection| pending_share_of(state).is_none()),
            ],
        );
        let full = tm_resume_try_send(name.as_str(), TrySend::Full);
        assert!(trace.iter().any(|entry| full == *entry), "try_send response missing from the trace: {trace:?}");
    }

    #[test]
    fn nonblocking_close_full_child_arms_stop_timeout() {
        let _guards = trace_guards();
        let mut network = traced();
        let blockfetch = network.stage("blockfetch", hold_blockfetch);
        let blockfetch_sender = blockfetch.sender();
        let blockfetch = network.wire_up(blockfetch, ());
        let connection = network.stage("connection", stage);
        let connection = network.wire_up(
            connection,
            established(Some(blockfetch_sender.contramap(Inputs::<BlockFetchMessage>::Local)), None),
        );

        let rt = Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.run(Run::default()).assert_idle();
        fill_to_capacity(&mut running, &blockfetch, || Inputs::Local(BlockFetchMessage::Close));
        running.trace_buffer().lock().clear();

        running.enqueue_msg(&connection, [ConnectionMessage::SetLocalUse(LocalUse::None)]);
        running.run(Run::default()).assert_sleeping();

        let state = running.get_state(&connection).expect("connection waited on a full close");
        let State::Established(established) = &state.state else {
            panic!("connection left Established");
        };
        assert_eq!(established.stopping, BTreeSet::from([ChildId::BlockFetch]));
        assert_eq!(established.desired_use, LocalUse::None);
        assert_eq!(established.actual_use, LocalUse::Diffusion);

        let name = connection.name().as_str();
        let delay = ManagerConfig::default().diffusion_stop_timeout;
        assert_trace_match_filter(
            &running,
            &[
                connection_input(name, |sent| matches!(sent, ConnectionMessage::SetLocalUse(LocalUse::None))),
                tm_try_send_match(name, "blockfetch", |sent: &Inputs<BlockFetchMessage>| {
                    matches!(sent, Inputs::Local(BlockFetchMessage::Close))
                }),
                tm_resume_try_send(name, TrySend::Full),
                stop_timeout(name, delay),
                tm_state_match(name, |state: &Connection| {
                    let State::Established(established) = &state.state else {
                        return false;
                    };
                    established.stopping == BTreeSet::from([ChildId::BlockFetch])
                }),
            ],
            &[drop_resume_except_try_send(), drop_other_stages(name)],
        );
    }

    fn stop_timeout(stage: &str, delay: Duration) -> TraceMatch<'static> {
        let stage = stage.to_string();
        let description = format!("SetTimeout(slot {STOP_TIMEOUT_SLOT}, {delay:?}, StopTimeout) at {stage}");
        TraceMatch::Property(
            Box::new(move |src| {
                let Some(Effect::SetTimeout { at_stage, slot, delay: got, msg }) = src.suspend() else {
                    return false;
                };
                at_stage.as_str() == stage
                    && *slot == STOP_TIMEOUT_SLOT
                    && *got == delay
                    && msg
                        .cast_ref::<ConnectionMessage>()
                        .is_ok_and(|message| matches!(message, ConnectionMessage::StopTimeout))
            }),
            description,
        )
    }
}
