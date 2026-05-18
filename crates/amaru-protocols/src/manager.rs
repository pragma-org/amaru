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

use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

use amaru_kernel::{EraHistory, NetworkMagic, Peer, Point, Tip};
use amaru_observability::trace_span;
use amaru_ouroboros::{ConnectionDirection, ConnectionId, MempoolMsg, ToSocketAddrs};
use pure_stage::{DeserializerGuards, Effects, Instant, StageRef, register_data_deserializer};
use tracing::Instrument;

use crate::{
    accept::{self, PullAccept},
    blockfetch::{Blocks, Blocks2},
    chainsync::ChainSyncInitiatorMsg,
    connection::{self, ConnectionMessage},
    network_effects::{ConnectError, Network, NetworkOps},
    protocol::Role,
};

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
    // TODO: remove, then rename FetchBlocks2 to FetchBlocks
    FetchBlocks {
        peer: Peer,
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
    },
    /// Fetch all blocks on the given chain fragment
    FetchBlocks2 {
        from: Point,
        through: Point,
        cr: StageRef<Blocks2>,
        id: u64,
    },
    /// Advertise this new tip to all downstream peers.
    NewTip(Tip),
    /// INTERNAL message sent by the connect handler stage after attempting a connection.
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
    },
}

impl ManagerMessage {
    fn message_type(&self) -> &'static str {
        match self {
            ManagerMessage::AddPeer(_) => "AddPeer",
            ManagerMessage::RemovePeer(_) => "RemovePeer",
            ManagerMessage::Disconnect(..) => "Disconnect",
            ManagerMessage::Listen(_) => "Listen",
            ManagerMessage::FetchBlocks { .. } => "FetchBlocks",
            ManagerMessage::FetchBlocks2 { .. } => "FetchBlocks2",
            ManagerMessage::NewTip(_) => "NewTip",
            ManagerMessage::ConnectionResult(..) => "ConnectionResult",
            ManagerMessage::ConnectionDied(..) => "ConnectionDied",
            ManagerMessage::Accepted(..) => "Accepted",
            ManagerMessage::HandshakeComplete { .. } => "HandshakeComplete",
        }
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
/// All connections are held in [`Manager::connections`] indexed by [`ConnectionId`].
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
/// When the connection dies, the manager will inform `peer_selection` about the disconnection
/// and then try to reconnect until successful or retries exhausted unless connections to this
/// peer have died thrice within [`ManagerConfig::three_strike_window`].
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
    connect_handler: StageRef<(Peer, Duration)>,
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

const MAX_OUTBOUND_DEATHS_TRACKED: usize = 2;

#[derive(Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct PeerState {
    outbound: OutboundState,
    inbound: Option<ConnectionId>,
    outbound_death_times: [Option<Instant>; MAX_OUTBOUND_DEATHS_TRACKED],
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
            connect_handler: StageRef::blackhole(),
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
    pub three_strike_window: Duration,
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

    pub fn with_accept_interval(mut self, accept_interval: Duration) -> Self {
        self.accept_interval = accept_interval;
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
            three_strike_window: Duration::from_secs(60),
        }
    }
}

impl Manager {
    async fn add_peer(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let state = self.peers.entry(peer.clone()).or_default();
        match &state.outbound {
            OutboundState::Connected { .. } | OutboundState::Scheduled { .. } => {
                tracing::info!(%peer, "discarding connection request, already connected or scheduled");
            }
            OutboundState::None => {
                tracing::info!(%peer, "adding peer");
                state.outbound = OutboundState::Scheduled { retries: self.config.connect_retries };
                self.connect(peer, true, eff).await;
            }
        }
    }

    async fn connect(&mut self, peer: Peer, immediate: bool, eff: &Effects<ManagerMessage>) {
        let (has_inbound, attempts) = match self.peers.get_mut(&peer) {
            Some(PeerState { outbound: OutboundState::Connected { .. }, .. }) => {
                tracing::debug!(%peer, "discarding connection request, already connected");
                return;
            }
            Some(PeerState { outbound: OutboundState::Scheduled { retries }, inbound, .. }) => {
                (inbound.is_some(), retries)
            }
            None | Some(PeerState { outbound: OutboundState::None, .. }) => {
                tracing::debug!(%peer, "discarding connection request, not added");
                return;
            }
        };
        if *attempts > 0 {
            *attempts -= 1;
            eff.ensure_child(&mut self.connect_handler, "connect_handler", connect_handler, || {
                ConnectHandler::new(self.config.connection_timeout, eff.me())
            })
            .await;
            let delay = if immediate { Duration::ZERO } else { self.config.reconnect_delay };
            eff.send(&self.connect_handler, (peer, delay)).await;
        } else {
            tracing::info!(%peer, "no more connection attempts left, removing peer");
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
                tracing::info!(%peer, %conn_id, "connection established");
                self.start_connection_stage(eff, peer, conn_id, ConnectionDirection::Outbound).await;
            }
            Err(err) => {
                tracing::error!(%peer, ?err, "connection failed");
                self.connect(peer, false, eff).await;
            }
        }
    }

    async fn listen(&mut self, listen_addr: SocketAddr, eff: &Effects<ManagerMessage>) {
        let network = Network::new(eff);
        match network.listen(listen_addr).await {
            Ok(listen_addr) => {
                tracing::info!(%listen_addr, "listening");
                let accept_stage = eff.stage("accept", accept::stage).await;
                let accept_stage = eff.supervise(accept_stage, ManagerMessage::Listen(listen_addr));
                let accept_stage =
                    eff.wire_up(accept_stage, accept::AcceptState::new(eff.me(), self.config(), listen_addr)).await;
                eff.send(&accept_stage, PullAccept).await;
            }
            Err(error) => {
                tracing::error!(%listen_addr, %error, "cannot listen");
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
        let connection = eff.supervise(connection, ManagerMessage::ConnectionDied(peer.clone(), conn_id, role));
        let connection = eff
            .wire_up(
                connection,
                connection::Connection::new(
                    peer.clone(),
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
        eff: &Effects<ManagerMessage>,
    ) {
        let direction = match role {
            Role::Initiator => ConnectionDirection::Outbound,
            Role::Responder => ConnectionDirection::Inbound,
        };
        tracing::info!(%peer, %conn_id, full_duplex_capable, full_duplex, "handshake completed");
        let peer_state = self.peers.entry(peer.clone()).or_default();
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
                    peer: peer.clone(),
                    conn_id,
                    direction,
                    full_duplex_capable,
                    full_duplex,
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
            tracing::info!(%peer, %conn_id, "handshake completed for duplicate connection, terminating it");
            eff.send(&stage, ConnectionMessage::Disconnect).await;
        }
    }

    #[expect(clippy::expect_used)]
    async fn remove_peer(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let Some(entry) = self.peers.remove(&peer) else {
            tracing::info!(%peer, "disconnect request ignored, not connected");
            return;
        };
        if let Some(conn_id) = entry.inbound {
            tracing::info!(%peer, %conn_id, "disconnecting inbound connection");
            let connection = self.connections.remove(&conn_id).expect("PeerState implies Connection");
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Disconnected {
                    peer: peer.clone(),
                    conn_id,
                    direction: ConnectionDirection::Inbound,
                    will_retry: false,
                },
            )
            .await;
            eff.send(&connection.stage, ConnectionMessage::Disconnect).await;
        }
        if let OutboundState::Connected { conn_id } = entry.outbound {
            tracing::info!(%peer, %conn_id, "disconnecting outbound connection");
            let connection = self.connections.remove(&conn_id).expect("PeerState implies Connection");
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Disconnected {
                    peer: peer.clone(),
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
            tracing::debug!(%peer, "connection died, peer already removed");
            return;
        };
        if let Some(Connection { direction, .. }) = self.connections.remove(&conn_id) {
            match direction {
                ConnectionDirection::Inbound => {
                    assert_eq!(peer_state.inbound, Some(conn_id));
                    assert_eq!(role, Role::Responder);
                    if peer_state.outbound == OutboundState::None {
                        tracing::info!(%peer, "inbound connection died, removing peer");
                        self.peers.remove(&peer);
                    } else {
                        tracing::info!(%peer, "inbound connection died, but outbound connection exists, keeping peer");
                        peer_state.inbound = None;
                    }
                }
                ConnectionDirection::Outbound => {
                    assert_eq!(peer_state.outbound, OutboundState::Connected { conn_id });
                    assert_eq!(role, Role::Initiator);
                    let now = eff.clock().await;
                    let times = &mut peer_state.outbound_death_times;
                    // rotate to the left, so the oldest is in last position, then replace last with current time
                    times.rotate_left(1);
                    if let Some(oldest) = times[const { MAX_OUTBOUND_DEATHS_TRACKED - 1 }].replace(now)
                        && now.saturating_since(oldest) < self.config.three_strike_window
                    {
                        tracing::info!(%peer, "outbound connection died; three strikes within window, suppressing retries");
                        if peer_state.inbound.is_none() {
                            self.peers.remove(&peer);
                        } else {
                            peer_state.outbound = OutboundState::None;
                        }
                        eff.send(&self.peer_selection, PeerSelectionNotify::ConnectFailed { peer: peer.clone() }).await;
                    } else {
                        tracing::info!(%peer, "outbound connection died, scheduling reconnect");
                        peer_state.outbound = OutboundState::Scheduled { retries: self.config.connect_retries };
                        self.connect(peer.clone(), false, eff).await;
                    }
                }
            }
            eff.send(
                &self.peer_selection,
                PeerSelectionNotify::Disconnected { peer, conn_id, direction, will_retry: role == Role::Initiator },
            )
            .await;
        } else {
            // pre-handshake death (no entry was inserted to connections, and no Connected notify was sent)
            tracing::debug!(%peer, %conn_id, "connection died before handshake completed");
            if role == Role::Initiator {
                self.connect(peer.clone(), false, eff).await;
            }
            // inbound pre-HS deaths require no further action (peer entry is only created on HS success)
        }
    }

    async fn fetch_blocks(
        &mut self,
        from: Point,
        through: Point,
        cr: StageRef<Blocks2>,
        id: u64,
        eff: &Effects<ManagerMessage>,
    ) {
        tracing::debug!(?from, ?through, "fetching blocks");
        let mut sent = 0;
        for conn in self.connections.values() {
            if !conn.may_initiate {
                continue;
            }
            eff.send(&conn.stage, ConnectionMessage::FetchBlocks2 { from, through, cr: cr.clone(), id }).await;
            sent += 1;
        }
        if sent == 0 {
            tracing::info!("no connections available to fetch blocks, returning empty result");
            eff.send(&cr, Blocks2::NoBlocks(id)).await;
        } else {
            tracing::debug!(%id, sent, "fetch blocks request sent to connections");
        }
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
    let span = trace_span!(amaru_observability::amaru::protocols::manager::MANAGER_STAGE, message_type = message_type);

    async move {
        match msg {
            ManagerMessage::AddPeer(peer) => {
                let span =
                    trace_span!(amaru_observability::amaru::protocols::manager::ADD_PEER, peer = peer.to_string());
                manager.add_peer(peer, &eff).instrument(span).await;
            }
            ManagerMessage::Accepted(peer, conn_id) => {
                let span = trace_span!(
                    amaru_observability::amaru::protocols::manager::ACCEPTED,
                    peer = peer.to_string(),
                    conn_id = conn_id.to_string()
                );
                manager.accepted(peer, conn_id, &eff).instrument(span).await;
            }
            ManagerMessage::RemovePeer(peer) => {
                let span =
                    trace_span!(amaru_observability::amaru::protocols::manager::REMOVE_PEER, peer = peer.to_string());
                manager.remove_peer(peer, &eff).instrument(span).await;
            }
            ManagerMessage::Disconnect(peer, conn_id) => {
                tracing::debug!(%peer, %conn_id, "disconnecting specific connection");
                if let Some(connection) = manager.connections.get(&conn_id) {
                    eff.send(&connection.stage, ConnectionMessage::Disconnect).await;
                } else {
                    tracing::debug!(%peer, %conn_id, "connection not found for disconnect");
                }
            }
            ManagerMessage::ConnectionDied(peer, conn_id, role) => {
                let span = trace_span!(
                    amaru_observability::amaru::protocols::manager::CONNECTION_DIED,
                    peer = peer.to_string(),
                    conn_id = conn_id.to_string(),
                    role = role.to_string()
                );
                manager.connection_died(peer, conn_id, role, &eff).instrument(span).await;
            }
            ManagerMessage::HandshakeComplete { peer, stage, conn_id, role, full_duplex_capable, full_duplex } => {
                manager.handshake_complete(peer, stage, conn_id, role, full_duplex_capable, full_duplex, &eff).await;
            }
            ManagerMessage::FetchBlocks { peer, from, through, cr } => {
                tracing::trace!(?from, ?through, %peer, "fetching blocks");
                if let Some(peer_state) = manager.peers.get(&peer) {
                    if let Some(conn_id) = peer_state.inbound
                        && let Some(connection) = manager.connections.get(&conn_id)
                        && connection.may_initiate
                    {
                        tracing::trace!(%peer, %conn_id, "fetching blocks on inbound connection");
                        eff.send(&connection.stage, ConnectionMessage::FetchBlocks { from, through, cr: cr.clone() })
                            .await;
                    }
                    if let OutboundState::Connected { conn_id } = peer_state.outbound
                        && let Some(connection) = manager.connections.get(&conn_id)
                        && connection.may_initiate
                    {
                        tracing::trace!(%peer, %conn_id, "fetching blocks on outbound connection");
                        eff.send(&connection.stage, ConnectionMessage::FetchBlocks { from, through, cr }).await;
                    }
                } else {
                    tracing::error!(%peer, "peer not found");
                    eff.send(&cr, Blocks::default()).await;
                }
            }
            ManagerMessage::Listen(listen_addr) => {
                manager.listen(listen_addr, &eff).await;
            }
            ManagerMessage::NewTip(tip) => {
                for conn in manager.connections.values() {
                    eff.send(&conn.stage, ConnectionMessage::NewTip(tip)).await;
                }
            }
            ManagerMessage::FetchBlocks2 { from, through, cr, id } => {
                manager.fetch_blocks(from, through, cr, id, &eff).await;
            }
            ManagerMessage::ConnectionResult(peer, conn_id) => {
                manager.connection_result(peer, conn_id, &eff).await;
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
        tracing::error!(?err, %peer, "failed to close connection");
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ConnectHandler {
    connection_timeout: Duration,
    manager: StageRef<ManagerMessage>,
}

impl ConnectHandler {
    fn new(connection_timeout: Duration, manager: StageRef<ManagerMessage>) -> Self {
        Self { connection_timeout, manager }
    }
}

async fn connect_handler(
    state: ConnectHandler,
    (peer, delay): (Peer, Duration),
    eff: Effects<(Peer, Duration)>,
) -> ConnectHandler {
    if delay > Duration::ZERO {
        eff.schedule_after((peer, Duration::ZERO), delay).await;
        return state;
    }
    let result = Network::new(&eff).connect(ToSocketAddrs::String(peer.to_string()), state.connection_timeout).await;
    eff.send(&state.manager, ManagerMessage::ConnectionResult(peer, result)).await;
    state
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        register_data_deserializer::<Manager>().boxed(),
        register_data_deserializer::<ManagerMessage>().boxed(),
        register_data_deserializer::<PeerSelectionNotify>().boxed(),
        register_data_deserializer::<Instant>().boxed(),
    ]
}
