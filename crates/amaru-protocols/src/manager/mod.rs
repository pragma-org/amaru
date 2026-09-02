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

use std::{collections::BTreeMap, net::SocketAddr, num::NonZeroUsize, sync::Arc, time::Duration};

use amaru_kernel::{EraHistory, NetworkMagic, Peer, Point};
use amaru_observability::{Instrument, TraceContext, debug, debug_span, error, info};
use amaru_ouroboros::{ConnectionDirection, ConnectionId, MempoolMsg};
use amaru_pure_stage::{DeserializerGuards, Effects, Instant, StageRef, register_data_deserializer};

use crate::{
    accept::{self, PullAccept},
    blockfetch::Blocks,
    chainsync::ChainSyncInitiatorMsg,
    connection::{self, ConnectionMessage},
    network_effects::{ConnectError, Network, NetworkOps},
    peer_sharing::{SharePeersReply, ShareResult},
    protocol::Role,
    tx_submission::ResponderParams,
};

pub mod connector;

/// Messages the [`Manager`] sends to the consensus `peer_selection` stage.
///
/// Notifications are sent *only after the handshake completes successfully*, so that
/// `full_duplex` status is known accurately.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSelectionNotify {
    /// A connection has been established and the handshake completed successfully.
    /// This is the only moment at which `peer_selection` learns about a usable connection.
    Connected {
        peer: Peer,
        conn_id: ConnectionId,
        direction: ConnectionDirection,
        full_duplex_capable: bool,
        full_duplex: bool,
        advertisable: bool,
    },

    /// A connection has been terminated (graceful disconnect, error, handshake refusal,
    /// or network error).
    ///
    /// If the connection was outbound then it may be retried, leading to either
    /// `ConnectFailed` or `Connected`.
    Disconnected { peer: Peer, conn_id: ConnectionId, direction: ConnectionDirection, will_retry: bool },

    /// An outbound connection attempt has failed (e.g. connection timeout, handshake refusal, network error)
    /// for a number of tries, see [`ManagerConfig::connect_retries`].
    ConnectFailed { peer: Peer },

    /// Inbound peer-sharing request: select addresses to advertise and reply on `reply_to`.
    ShareRequest { peer: Peer, amount: u8, reply_to: StageRef<SharePeersReply> },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManagerMessage {
    /// Start outgoing connection attempts to the given peer until successful or retries exhausted.
    ///
    /// If the connection succeeds then future disconnection will first lead to retries before giving up.
    AddPeer(Peer),
    /// Remove a peer and terminate all of its connections.
    RemovePeer(Peer),
    /// Terminate the given connection only.
    Disconnect(Peer, ConnectionId),
    /// Start listening for incoming connections on the given socket address.
    Listen(SocketAddr),
    /// Fetch blocks on the given chain fragment.
    ///
    /// When `peers` is `Some`, only those peers' initiating connections are asked.
    /// When `None`, every initiating connection is asked (cold-start / empty-selection fallback).
    FetchBlocks { from: Point, through: Point, cr: StageRef<Blocks>, id: u64, peers: Option<Vec<Peer>> },
    /// Start periodic peer-sharing requests on one outbound connection.
    ///
    /// The initiator schedules the first request after `initial_delay`, then every `interval`
    /// after each reply. Results are delivered on `reply_to` until the connection ends.
    /// If no initiating connection exists, an empty [`ShareResult`] is sent once.
    RequestSharePeers {
        peer: Peer,
        amount: u8,
        initial_delay: std::time::Duration,
        interval: std::time::Duration,
        reply_to: StageRef<ShareResult>,
    },
    /// Server-side peer-sharing: ask peer selection for addresses to return to `peer`.
    ShareRequest { peer: Peer, amount: u8, reply_to: StageRef<SharePeersReply> },
    /// Advertise this new tip to all downstream peers.
    NewTip(Point, TraceContext),
    /// INTERNAL message sent by the connector stage after a connection attempt completes.
    ConnectionResult(Peer, Result<ConnectionId, ConnectError>),
    /// INTERNAL message sent from the connection stage only!
    ///
    /// Must contain the connection ID so that we can then close the actual socket;
    /// the `peers` entry could already have been removed by RemovePeer.
    // TODO move to separate message type
    ConnectionDied(Peer, ConnectionId, Role),
    /// INTERNAL message sent by the accept stage after accepting a new connection.
    Accepted(Peer, ConnectionId),
    /// INTERNAL Sent by the connection stage after successful handshake.
    /// This allows the manager to notify peer_selection with accurate full_duplex status.
    HandshakeComplete {
        peer: Peer,
        stage: StageRef<ConnectionMessage>,
        conn_id: ConnectionId,
        role: Role,
        full_duplex_capable: bool,
        full_duplex: bool,
        advertisable: bool,
    },
    /// Ask a live connection to converge toward this local use.
    SetLocalUse { peer: Peer, conn_id: ConnectionId, local_use: crate::connection::LocalUse },
}

impl ManagerMessage {
    fn message_type(&self) -> &'static str {
        match self {
            ManagerMessage::AddPeer(_) => "AddPeer",
            ManagerMessage::RemovePeer(_) => "RemovePeer",
            ManagerMessage::Disconnect(..) => "Disconnect",
            ManagerMessage::Listen(_) => "Listen",
            ManagerMessage::FetchBlocks { .. } => "FetchBlocks",
            ManagerMessage::RequestSharePeers { .. } => "RequestSharePeers",
            ManagerMessage::ShareRequest { .. } => "ShareRequest",
            ManagerMessage::NewTip(_, _) => "NewTip",
            ManagerMessage::ConnectionResult(..) => "ConnectionResult",
            ManagerMessage::ConnectionDied(..) => "ConnectionDied",
            ManagerMessage::Accepted(..) => "Accepted",
            ManagerMessage::HandshakeComplete { .. } => "HandshakeComplete",
            ManagerMessage::SetLocalUse { .. } => "SetLocalUse",
        }
    }

    pub fn new_tip(tip: Point) -> Self {
        ManagerMessage::NewTip(tip, TraceContext::none())
    }
}

/// The manager stage is responsible for managing the connections to the peers.
///
/// It is important to keep in mind that inbound connections are controlled by the peer
/// and that the peer may bind the socket to a specific port before connecting. If this
/// manager listens on multiple IP addresses, then it is possible for the same peer to
/// open multiple inbound connections from the same remote SocketAddr, hence the same
/// [`Peer`]. These connections are distinguished by their [`ConnectionId`].
///
/// Outbound connections are controlled and to a given peer only one may be initiated at
/// a time. The [`Peer`] we connect to may also show up as inbound, therefore we need to
/// keep these separate.
///
/// ## Design
///
/// All connections are held in `Manager::connections` indexed by [`ConnectionId`].
/// For each peer we keep track of the outbound state (which is `None` in case no
/// outbound connection has been requested) and up to one inbound connection.
/// If a second connection comes in from the same peer, this new connection will be
/// terminated (the handshake will be run, sending [`crate::protocol_messages::handshake::RefuseReason::Refused`]).
///
/// An inbound connection is accepted (subject to connection limits and the above) and
/// after successful handshake the manager notifies `peer_selection` about the new connection.
/// When the connection dies, there are no retries and the manager immediately notifies
/// `peer_selection` about the disconnection.
///
/// An outbound connection is initiated by sending `ManagerMessage::AddPeer`. The manager will
/// then try to connect to that peer until successful or retries exhausted. After a successful
/// connection and handshake, the manager notifies `peer_selection` about the new connection.
/// When the connection dies, the manager notifies `peer_selection` and does **not** redial.
/// Peer selection decides whether to `AddPeer` again.
///
/// ## Behavioural contracts
///
/// - [`PeerSelectionNotify::Connected`] is always paired with a future [`PeerSelectionNotify::Disconnected`]
///   for the same `peer` and `conn_id`.
///
///   This also holds true if [`ManagerMessage::RemovePeer`] is processed between.
///
/// - Sending [`ManagerMessage::AddPeer`] will generate [`PeerSelectionNotify::ConnectFailed`]
///   if the connection cannot be (re)established before [`ManagerMessage::RemovePeer`] is
///   received.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Manager {
    peers: BTreeMap<Peer, PeerState>,
    connections: BTreeMap<ConnectionId, Connection>,
    connector: StageRef<connector::ConnectorMsg>,
    magic: NetworkMagic,
    config: ManagerConfig,
    era_history: Arc<EraHistory>,
    chain_sync: StageRef<ChainSyncInitiatorMsg>,
    mempool: StageRef<MempoolMsg>,
    peer_selection: StageRef<PeerSelectionNotify>,
}

#[derive(Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum OutboundState {
    #[default]
    None,
    Scheduled {
        retries: u16,
    },
    Connected {
        conn_id: ConnectionId,
    },
}

#[derive(Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct PeerState {
    outbound: OutboundState,
    inbound: Option<ConnectionId>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Connection {
    peer: Peer,
    stage: StageRef<ConnectionMessage>,
    direction: ConnectionDirection,
    /// Whether we may initiate mini-protocols on this connection.
    ///
    /// This is false while the handshake is ongoing and may remain
    /// false afterwards, e.g. if inbound and not full-duplex capable.
    may_initiate: bool,
    full_duplex_capable: bool,
}

impl Manager {
    pub fn new(
        magic: NetworkMagic,
        config: ManagerConfig,
        era_history: Arc<EraHistory>,
        chain_sync: StageRef<ChainSyncInitiatorMsg>,
        mempool: StageRef<MempoolMsg>,
        peer_selection: StageRef<PeerSelectionNotify>,
    ) -> Self {
        Self {
            peers: BTreeMap::new(),
            connections: BTreeMap::new(),
            connector: StageRef::blackhole(),
            magic,
            config,
            era_history,
            chain_sync,
            mempool,
            peer_selection,
        }
    }

    pub fn config(&self) -> ManagerConfig {
        self.config
    }
}

/// Parameters for the Manager: connection timeout, reconnection delay, etc...
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerConfig {
    pub connection_timeout: Duration,
    pub reconnect_delay: Duration,
    pub connect_retries: u16,
    pub accept_interval: Duration,
    pub tx_submission_params: ResponderParams,
    /// When `Some`, the BlockFetch initiator uses N lock-step typestate instances
    /// and a fused pipeline cursor. `None` keeps the lock-step `miniprotocol` handler.
    pub blockfetch_pipeline_n: Option<NonZeroUsize>,
}

impl ManagerConfig {
    pub fn with_reconnect_delay(mut self, reconnect_delay: Duration) -> Self {
        self.reconnect_delay = reconnect_delay;
        self
    }

    pub fn with_connection_timeout(mut self, connection_timeout: Duration) -> Self {
        self.connection_timeout = connection_timeout;
        self
    }

    pub fn with_connect_retries(mut self, retries: u16) -> Self {
        self.connect_retries = retries;
        self
    }

    pub fn with_accept_interval(mut self, accept_interval: Duration) -> Self {
        self.accept_interval = accept_interval;
        self
    }

    pub fn with_tx_submission_params(mut self, params: ResponderParams) -> Self {
        self.tx_submission_params = params;
        self
    }

    pub fn with_blockfetch_pipeline_n(mut self, n: Option<NonZeroUsize>) -> Self {
        self.blockfetch_pipeline_n = n;
        self
    }
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            connection_timeout: Duration::from_secs(10),
            reconnect_delay: Duration::from_secs(2),
            connect_retries: 3,
            accept_interval: Duration::from_millis(100),
            tx_submission_params: ResponderParams::default(),
            blockfetch_pipeline_n: None,
        }
    }
}

impl Manager {
    async fn add_peer(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let state = self.peers.entry(peer).or_default();
        match &state.outbound {
            OutboundState::Connected { .. } | OutboundState::Scheduled { .. } => {
                info!(protocols::manager::peer::CONNECT_DISCARDED, peer, reason = "already_connected_or_scheduled");
            }
            OutboundState::None => {
                info!(protocols::manager::peer::CONNECT, peer);
                state.outbound = OutboundState::Scheduled { retries: self.config.connect_retries };
                self.connect(peer, true, eff).await;
            }
        }
    }

    async fn connect(&mut self, peer: Peer, immediate: bool, eff: &Effects<ManagerMessage>) {
        let (has_inbound, attempts) = match self.peers.get_mut(&peer) {
            Some(PeerState { outbound: OutboundState::Connected { .. }, .. }) => {
                debug!(protocols::manager::peer::CONNECT_DISCARDED, peer, reason = "already_connected");
                return;
            }
            Some(PeerState { outbound: OutboundState::Scheduled { retries }, inbound, .. }) => {
                (inbound.is_some(), retries)
            }
            None | Some(PeerState { outbound: OutboundState::None, .. }) => {
                debug!(protocols::manager::peer::CONNECT_DISCARDED, peer, reason = "not_added");
                return;
            }
        };
        if *attempts > 0 {
            *attempts -= 1;
            eff.ensure_child(&mut self.connector, "connector", connector::stage, || {
                connector::Connector::new(self.config.connection_timeout, eff.me())
            })
            .await;
            let delay = if immediate { Duration::ZERO } else { self.config.reconnect_delay };
            eff.send(&self.connector, connector::ConnectorMsg::Connect { peer, delay }).await;
        } else {
            info!(protocols::manager::peer::CONNECT_EXHAUSTED, peer);
            if !has_inbound {
                self.peers.remove(&peer);
            } else if let Some(state) = self.peers.get_mut(&peer) {
                state.outbound = OutboundState::None;
            }
            eff.send(&self.peer_selection, PeerSelectionNotify::ConnectFailed { peer }).await;
        }
    }

    async fn connection_result(
        &mut self,
        peer: Peer,
        result: Result<ConnectionId, ConnectError>,
        eff: &Effects<ManagerMessage>,
    ) {
        match result {
            Ok(conn_id) => {
                info!(protocols::manager::peer::CONNECTED, peer, conn_id = conn_id.as_u64());
                self.start_connection_stage(eff, peer, conn_id, ConnectionDirection::Outbound).await;
            }
            Err(err) => {
                info!(protocols::manager::peer::CONNECT_FAILED, peer, error = err.to_string());
                self.connect(peer, false, eff).await;
            }
        }
    }

    async fn listen(&mut self, listen_addr: SocketAddr, eff: &Effects<ManagerMessage>) {
        let network = Network::new(eff);
        match network.listen(listen_addr).await {
            Ok(listen_addr) => {
                info!(protocols::manager::listen::STARTED, listen_addr = listen_addr.to_string());
                let accept_stage = eff.stage("accept", accept::stage).await;
                let accept_stage = eff.supervise(accept_stage, ManagerMessage::Listen(listen_addr));
                let accept_stage =
                    eff.wire_up(accept_stage, accept::AcceptState::new(eff.me(), self.config(), listen_addr)).await;
                eff.send(&accept_stage, PullAccept).await;
            }
            Err(error) => {
                error!(
                    protocols::manager::listen::FAILED,
                    listen_addr = listen_addr.to_string(),
                    error = error.to_string()
                );
                return eff.terminate().await;
            }
        }
    }

    async fn accepted(&mut self, peer: Peer, conn_id: ConnectionId, eff: &Effects<ManagerMessage>) {
        // Always start a connection stage for every accepted inbound. Duplicate detection (to keep at
        // most one inbound per peer) is performed after handshake success; extras are terminated then.
        // This ensures the handshake is always run (as documented) for all accepted connections.
        self.start_connection_stage(eff, peer, conn_id, ConnectionDirection::Inbound).await;
    }

    /// Start a stage to handle the connection lifecycle.
    async fn start_connection_stage(
        &mut self,
        eff: &Effects<ManagerMessage>,
        peer: Peer,
        conn_id: ConnectionId,
        direction: ConnectionDirection,
    ) {
        let connection = eff.stage(format!("{conn_id}-{peer}"), connection::stage).await;
        let role = match direction {
            ConnectionDirection::Inbound => Role::Responder,
            ConnectionDirection::Outbound => Role::Initiator,
        };
        let connection = eff.supervise(connection, ManagerMessage::ConnectionDied(peer, conn_id, role));
        let connection = eff
            .wire_up(
                connection,
                connection::Connection::new(
                    peer,
                    conn_id,
                    role,
                    self.config,
                    self.magic,
                    self.chain_sync.clone(),
                    self.era_history.clone(),
                    self.mempool.clone(),
                    eff.me(), // manager itself to receive HandshakeComplete
                ),
            )
            .await;
        eff.send(&connection, ConnectionMessage::Initialize).await;
    }

    #[expect(clippy::too_many_arguments)]
    async fn handshake_complete(
        &mut self,
        peer: Peer,
        stage: StageRef<ConnectionMessage>,
        conn_id: ConnectionId,
        role: Role,
        full_duplex_capable: bool,
        full_duplex: bool,
        advertisable: bool,
        eff: &Effects<ManagerMessage>,
    ) {
        let direction = match role {
            Role::Initiator => ConnectionDirection::Outbound,
            Role::Responder => ConnectionDirection::Inbound,
        };
        info!(
            protocols::manager::peer::HANDSHAKE_COMPLETED,
            peer,
            conn_id = conn_id.as_u64(),
            full_duplex_capable,
            full_duplex,
            advertisable
        );
        let peer_state = self.peers.entry(peer).or_default();
        let accept_this = match direction {
            ConnectionDirection::Outbound => {
                if matches!(peer_state.outbound, OutboundState::Connected { .. }) {
                    false
                } else {
                    peer_state.outbound = OutboundState::Connected { conn_id };
                    true
                }
            }
            ConnectionDirection::Inbound => {
                if peer_state.inbound.is_some() {
                    false
                } else {
                    peer_state.inbound = Some(conn_id);
                    true
                }
            }
        };
        if accept_this {
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Connected {
                    peer,
                    conn_id,
                    direction,
                    full_duplex_capable,
                    full_duplex,
                    advertisable,
                },
            )
            .await;
            self.connections.insert(
                conn_id,
                Connection {
                    stage,
                    direction,
                    full_duplex_capable,
                    peer,
                    may_initiate: role == Role::Initiator || full_duplex,
                },
            );
        } else {
            info!(protocols::manager::peer::DUPLICATE_TERMINATED, peer, conn_id = conn_id.as_u64());
            eff.send(&stage, ConnectionMessage::Disconnect).await;
        }
    }

    #[expect(clippy::expect_used)]
    async fn remove_peer(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let Some(entry) = self.peers.remove(&peer) else {
            info!(protocols::manager::peer::DISCONNECT_IGNORED, peer, reason = "not_connected");
            return;
        };
        if let Some(conn_id) = entry.inbound {
            info!(protocols::manager::peer::DISCONNECTING, peer, conn_id = conn_id.as_u64(), direction = "inbound");
            let connection = self.connections.remove(&conn_id).expect("PeerState implies Connection");
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Disconnected {
                    peer,
                    conn_id,
                    direction: ConnectionDirection::Inbound,
                    will_retry: false,
                },
            )
            .await;
            eff.send(&connection.stage, ConnectionMessage::Disconnect).await;
        }
        if let OutboundState::Connected { conn_id } = entry.outbound {
            info!(protocols::manager::peer::DISCONNECTING, peer, conn_id = conn_id.as_u64(), direction = "outbound");
            let connection = self.connections.remove(&conn_id).expect("PeerState implies Connection");
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Disconnected {
                    peer,
                    conn_id,
                    direction: ConnectionDirection::Outbound,
                    will_retry: false,
                },
            )
            .await;
            eff.send(&connection.stage, ConnectionMessage::Disconnect).await;
        }
    }

    async fn connection_died(&mut self, peer: Peer, conn_id: ConnectionId, role: Role, eff: &Effects<ManagerMessage>) {
        // this is needed to clean up the socket in case the connection stage errored out
        close_connection(eff, &peer, conn_id).await;
        let Some(peer_state) = self.peers.get_mut(&peer) else {
            debug!(protocols::manager::peer::DISCONNECT_IGNORED, peer, reason = "peer_already_removed");
            return;
        };
        if let Some(Connection { direction, .. }) = self.connections.remove(&conn_id) {
            match direction {
                ConnectionDirection::Inbound => {
                    assert_eq!(peer_state.inbound, Some(conn_id));
                    assert_eq!(role, Role::Responder);
                    if peer_state.outbound == OutboundState::None {
                        info!(protocols::manager::peer::CONNECTION_DIED_HANDLED, peer, outcome = "peer_removed");
                        self.peers.remove(&peer);
                    } else {
                        info!(protocols::manager::peer::CONNECTION_DIED_HANDLED, peer, outcome = "kept_for_outbound");
                        peer_state.inbound = None;
                    }
                }
                ConnectionDirection::Outbound => {
                    assert_eq!(peer_state.outbound, OutboundState::Connected { conn_id });
                    assert_eq!(role, Role::Initiator);
                    info!(protocols::manager::peer::CONNECTION_DIED_HANDLED, peer, outcome = "peer_selection_redial");
                    if peer_state.inbound.is_none() {
                        self.peers.remove(&peer);
                    } else {
                        peer_state.outbound = OutboundState::None;
                    }
                }
            }
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Disconnected { peer, conn_id, direction, will_retry: false },
            )
            .await;
        } else {
            // pre-handshake death (no entry was inserted to connections, and no Connected notify was sent)
            debug!(
                protocols::manager::peer::DISCONNECT_IGNORED,
                peer,
                reason = "before_handshake",
                conn_id = conn_id.as_u64()
            );
            if role == Role::Initiator {
                if let Some(state) = self.peers.get_mut(&peer) {
                    state.outbound = OutboundState::None;
                    if state.inbound.is_none() {
                        self.peers.remove(&peer);
                    }
                }
                eff.send(&self.peer_selection, PeerSelectionNotify::ConnectFailed { peer }).await;
            }
            // inbound pre-HS deaths require no further action (peer entry is only created on HS success)
        }
    }

    async fn fetch_blocks(
        &mut self,
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
        id: u64,
        peers: Option<Vec<Peer>>,
        eff: &Effects<ManagerMessage>,
    ) {
        debug!(protocols::manager::blocks::FETCH, from, through, peers = format!("{peers:?}"));
        let mut contacted = Vec::new();
        match peers {
            None => {
                for conn in self.connections.values() {
                    if !conn.may_initiate {
                        continue;
                    }
                    contacted.push(conn.peer);
                    eff.send(&conn.stage, ConnectionMessage::FetchBlocks { from, through, cr: cr.clone(), id }).await;
                }
            }
            Some(wanted) => {
                for peer in wanted {
                    let Some(conn) = self.connections.values().find(|c| c.may_initiate && c.peer == peer) else {
                        continue;
                    };
                    contacted.push(peer);
                    eff.send(&conn.stage, ConnectionMessage::FetchBlocks { from, through, cr: cr.clone(), id }).await;
                }
            }
        }
        if contacted.is_empty() {
            debug!(protocols::manager::blocks::FETCH_NO_PEERS, id);
            eff.send(&cr, Blocks::NoPeersAvailable(id)).await;
        } else {
            debug!(protocols::manager::blocks::FETCH_SENT, id, sent = contacted.len());
            eff.send(&cr, Blocks::PeersAsked(id, contacted)).await;
        }
    }

    async fn request_share_peers(
        &self,
        peer: Peer,
        amount: u8,
        initial_delay: std::time::Duration,
        interval: std::time::Duration,
        reply_to: StageRef<ShareResult>,
        eff: &Effects<ManagerMessage>,
    ) {
        let Some(conn) = self.connections.values().find(|c| c.may_initiate && c.peer == peer) else {
            debug!(protocols::manager::sharing::REQUEST_NO_CONNECTION, peer);
            eff.send(&reply_to, ShareResult { peer, peers: Vec::new() }).await;
            return;
        };
        eff.send(&conn.stage, ConnectionMessage::RequestSharePeers { amount, initial_delay, interval, reply_to }).await;
    }
}

/// The manager stage is responsible for managing the connections to the peers.
///
/// The semantics of the operations are as follows:
/// - AddPeer: add a peer to the manager unless that peer is already added
/// - RemovePeer: remove a peer from the manager, which will terminate a connection if currently connected
///
/// A peer can be added right after being removed even though the socket will be closed asynchronously.
pub async fn stage(mut manager: Manager, msg: ManagerMessage, eff: Effects<ManagerMessage>) -> Manager {
    let message_type = msg.message_type().to_string();
    let span = debug_span!(protocols::manager::message::PROCESS, message_type);

    async move {
        match msg {
            ManagerMessage::AddPeer(peer) => {
                let span = debug_span!(protocols::manager::peer::ADD, peer);
                manager.add_peer(peer, &eff).instrument(span).await;
            }
            ManagerMessage::Accepted(peer, conn_id) => {
                let span = debug_span!(protocols::manager::peer::ACCEPTED, peer, conn_id = conn_id.as_u64());
                manager.accepted(peer, conn_id, &eff).instrument(span).await;
            }
            ManagerMessage::RemovePeer(peer) => {
                let span = debug_span!(protocols::manager::peer::REMOVE, peer);
                manager.remove_peer(peer, &eff).instrument(span).await;
            }
            ManagerMessage::Disconnect(peer, conn_id) => {
                debug!(
                    protocols::manager::peer::DISCONNECTING,
                    peer,
                    conn_id = conn_id.as_u64(),
                    direction = "requested"
                );
                if let Some(connection) = manager.connections.get(&conn_id) {
                    eff.send(&connection.stage, ConnectionMessage::Disconnect).await;
                } else {
                    debug!(
                        protocols::manager::peer::DISCONNECT_IGNORED,
                        peer,
                        reason = "connection_not_found",
                        conn_id = conn_id.as_u64()
                    );
                }
            }
            ManagerMessage::ConnectionDied(peer, conn_id, role) => {
                let span = debug_span!(
                    protocols::manager::peer::CONNECTION_DIED,
                    peer,
                    conn_id = conn_id.as_u64(),
                    role = role.to_string(),
                );
                manager.connection_died(peer, conn_id, role, &eff).instrument(span).await;
            }
            ManagerMessage::HandshakeComplete {
                peer,
                stage,
                conn_id,
                role,
                full_duplex_capable,
                full_duplex,
                advertisable,
            } => {
                manager
                    .handshake_complete(
                        peer,
                        stage,
                        conn_id,
                        role,
                        full_duplex_capable,
                        full_duplex,
                        advertisable,
                        &eff,
                    )
                    .await;
            }
            ManagerMessage::Listen(listen_addr) => {
                manager.listen(listen_addr, &eff).await;
            }
            ManagerMessage::NewTip(tip, trace_context) => {
                for conn in manager.connections.values() {
                    eff.send(&conn.stage, ConnectionMessage::NewTip(tip, trace_context.clone())).await;
                }
            }
            ManagerMessage::FetchBlocks { from, through, cr, id, peers } => {
                manager.fetch_blocks(from, through, cr, id, peers, &eff).await;
            }
            ManagerMessage::RequestSharePeers { peer, amount, initial_delay, interval, reply_to } => {
                manager.request_share_peers(peer, amount, initial_delay, interval, reply_to, &eff).await;
            }
            ManagerMessage::ShareRequest { peer, amount, reply_to } => {
                eff.send(&manager.peer_selection, PeerSelectionNotify::ShareRequest { peer, amount, reply_to }).await;
            }
            ManagerMessage::ConnectionResult(peer, conn_id) => {
                manager.connection_result(peer, conn_id, &eff).await;
            }
            ManagerMessage::SetLocalUse { peer: _, conn_id, local_use } => {
                if let Some(connection) = manager.connections.get(&conn_id) {
                    eff.send(&connection.stage, ConnectionMessage::SetLocalUse(local_use)).await;
                }
            }
        }
        manager
    }
    .instrument(span)
    .await
}

/// Close the connection and log any errors.
async fn close_connection(eff: &Effects<ManagerMessage>, peer: &Peer, conn_id: ConnectionId) {
    if let Err(err) = Network::new(eff).close(conn_id).await {
        error!(protocols::manager::peer::CLOSE_FAILED, peer, error = err.to_string());
    }
}

pub fn register_deserializers() -> DeserializerGuards {
    let mut guards = vec![
        register_data_deserializer::<Manager>().boxed(),
        register_data_deserializer::<ManagerMessage>().boxed(),
        register_data_deserializer::<PeerSelectionNotify>().boxed(),
        register_data_deserializer::<Instant>().boxed(),
    ];
    guards.extend(connector::register_deserializers());
    guards
}
