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

use std::{collections::BTreeMap, net::SocketAddr, num::NonZeroU8, sync::Arc, time::Duration};

use amaru_kernel::{EraHistory, NetworkMagic, Peer, Point};
use amaru_observability::{ContinuedHeader, Instrument, TraceContext, debug, debug_span, error, info};
use amaru_ouroboros::{CloseReason, ConnectionDirection, ConnectionId, ConnectionRecord, MempoolMsg};
use amaru_pure_stage::{DeserializerGuards, Effects, Instant, StageRef, register_data_deserializer};

use crate::{
    accept::{self, PullAccept},
    blockfetch::Blocks,
    chainsync::ChainSyncInitiatorMsg,
    connection::{self, ConnectionMessage, LocalUse},
    network_effects::{ConnectError, Network, NetworkOps},
    peer_sharing::{SHARE_REQUEST_AMOUNT, SHARE_REQUEST_INITIAL_DELAY, SHARE_REQUEST_INTERVAL},
    peer_tracking::observed_at,
    peer_tracking_effects::PeerTrack,
    protocol::Role,
    protocol_messages::version_number::VersionNumber,
    tx_submission::ResponderParams,
};

pub mod connector;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManagerMessage {
    /// Start one outbound connection attempt to the given peer.
    ///
    /// A failed attempt is written to the peer-tracking resource and is not retried.
    /// After a successful session dies, peer selection issues a new `AddPeer`; the manager does not redial.
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
    /// Advertise this new tip to all downstream peers.
    NewTip(Point, TraceContext<ContinuedHeader>),
    /// INTERNAL message sent by the connector stage after a connection attempt completes.
    ///
    /// `attempt` is the dial [`AddPeer`](ManagerMessage::AddPeer) accepted. A connect error has
    /// no [`ConnectionId`]; the attempt number is what ties it to that dial. Two attempts to the
    /// same peer can finish out of order.
    ConnectionResult { peer: Peer, attempt: u64, result: Result<ConnectionId, ConnectError> },
    /// INTERNAL message sent from the connection stage only!
    ///
    /// Must contain the connection ID so that we can then close the actual socket;
    /// the `peers` entry could already have been removed by RemovePeer.
    // TODO move to separate message type
    ConnectionDied(Peer, ConnectionId, Role),
    /// INTERNAL message sent by the accept stage after accepting a new connection.
    Accepted(Peer, ConnectionId),
    /// INTERNAL Sent by the connection stage after a successful handshake.
    /// The manager records the bearer, including full-duplex status, on the performance resource.
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
    SetLocalUse { peer: Peer, conn_id: ConnectionId, local_use: LocalUse },
}

impl ManagerMessage {
    fn message_type(&self) -> &'static str {
        match self {
            ManagerMessage::AddPeer(_) => "AddPeer",
            ManagerMessage::RemovePeer(_) => "RemovePeer",
            ManagerMessage::Disconnect(..) => "Disconnect",
            ManagerMessage::Listen(_) => "Listen",
            ManagerMessage::FetchBlocks { .. } => "FetchBlocks",
            ManagerMessage::NewTip(_, _) => "NewTip",
            ManagerMessage::ConnectionResult { .. } => "ConnectionResult",
            ManagerMessage::ConnectionDied(..) => "ConnectionDied",
            ManagerMessage::Accepted(..) => "Accepted",
            ManagerMessage::HandshakeComplete { .. } => "HandshakeComplete",
            ManagerMessage::SetLocalUse { .. } => "SetLocalUse",
        }
    }

    pub fn new_tip(tip: Point) -> Self {
        ManagerMessage::NewTip(tip, TraceContext::detached())
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
/// An inbound connection is accepted when this peer has no inbound bearer yet and the inbound
/// cap is not full. A handshake past the cap is refused: nothing is written, and the bearer is
/// disconnected. When a connection dies, there are no retries.
///
/// An outbound connection is initiated by sending `ManagerMessage::AddPeer`. The manager makes
/// one attempt. If it fails, the failure is written to the peer-tracking resource and the manager
/// does not retry. After a successful connection and handshake, the bearer is written the same way.
/// When the connection dies, the close is written and the manager does **not** redial. Peer
/// selection decides whether to `AddPeer` again.
///
/// A message that names a [`ConnectionId`] is applied only while that id is still outstanding:
/// the handshake has not finished, or the bearer is live. A handshake, death, disconnect, or
/// local-use message for any other id is ignored. It does not record a connect failure, a close,
/// or a new bearer.
///
/// The socket is closed when this manager asks the connection stage to stop, and again when that
/// stage has terminated. The second close may find the socket already gone; that error is logged
/// at debug. A connect error carries no connection id. It counts only for the `AddPeer` attempt
/// that is still outstanding. A success for an attempt that was dropped closes that socket and
/// does not start a connection stage.
///
/// A second handshake for a peer that already has a bearer in that direction does not write
/// another bearer. When that extra bearer later dies, including after the outbound slot has
/// moved on, the manager does not record a connect failure. The remaining bearer stays.
///
/// Each lifecycle fact is one worker operation: handshake (`record_connection_established`),
/// a close, and a failed outbound attempt. Outbound local use at handshake is
/// [`LocalUse::Diffusion`] and inbound is [`LocalUse::None`]. The connection records a later
/// applied use on the peer resource. Fetch reads which bearers may initiate from there.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Manager {
    peers: BTreeMap<Peer, PeerState>,
    connections: BTreeMap<ConnectionId, Connection>,
    /// Sockets whose connection stage is running and whose handshake has not finished.
    pending: BTreeMap<ConnectionId, PendingConn>,
    /// Next [`OutboundState::Scheduled`] attempt. Starts at 1.
    next_attempt: u64,
    connector: StageRef<connector::ConnectorMsg>,
    magic: NetworkMagic,
    config: ManagerConfig,
    era_history: Arc<EraHistory>,
    chain_sync: StageRef<ChainSyncInitiatorMsg>,
    mempool: StageRef<MempoolMsg>,
}

#[derive(Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum OutboundState {
    #[default]
    None,
    /// `AddPeer` was accepted. `attempt` identifies this dial. `conn_id` is set once the socket
    /// exists and the connection stage is running, still before handshake.
    Scheduled {
        attempt: u64,
        conn_id: Option<ConnectionId>,
    },
    Connected {
        conn_id: ConnectionId,
    },
}

/// A socket whose connection stage is running and has not finished handshake.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct PendingConn {
    peer: Peer,
    stage: StageRef<ConnectionMessage>,
    direction: ConnectionDirection,
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
    full_duplex_capable: bool,
}

impl Manager {
    pub fn new(
        magic: NetworkMagic,
        config: ManagerConfig,
        era_history: Arc<EraHistory>,
        chain_sync: StageRef<ChainSyncInitiatorMsg>,
        mempool: StageRef<MempoolMsg>,
    ) -> Self {
        Self {
            peers: BTreeMap::new(),
            connections: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_attempt: 1,
            connector: StageRef::blackhole(),
            magic,
            config,
            era_history,
            chain_sync,
            mempool,
        }
    }

    pub fn config(&self) -> ManagerConfig {
        self.config
    }
}

/// Parameters for the Manager: connection timeout, reconnection delay, etc...
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerConfig {
    /// How long one outbound TCP connect attempt may run before it fails.
    pub connection_timeout: Duration,
    pub reconnect_delay: Duration,
    pub accept_interval: Duration,
    pub tx_submission_params: ResponderParams,
    /// BlockFetch initiator pipeline depth. `1` drives the lock-step typestate
    /// instance; values greater than 1 wrap N instances in the CIP-0164 pipeliner.
    pub blockfetch_pipeline_n: NonZeroU8,
    /// Last-to-finish bound when stopping the diffusion initiator group.
    pub diffusion_stop_timeout: Duration,
    /// Last-to-finish bound when stopping the maintenance initiator group.
    pub maintenance_stop_timeout: Duration,
    /// Highest node-to-node protocol version offered in handshake.
    ///
    /// Defaults to [`VersionNumber::CURRENT`] (V15). Tests pin V14 to check fallback.
    pub max_n2n_version: VersionNumber,
    /// Live inbound bearers accepted at handshake. Further inbound handshakes are refused.
    pub max_inbound: usize,
    /// Delay after the maintenance group starts before the first share request.
    pub share_request_initial_delay: Duration,
    /// Delay between a share reply and the next request.
    pub share_request_interval: Duration,
    /// Addresses requested on each share call.
    pub share_request_amount: u8,
}

/// Default inbound cap. The node sets this from its downstream peer target.
pub const DEFAULT_MAX_INBOUND: usize = 10;

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

    pub fn with_tx_submission_params(mut self, params: ResponderParams) -> Self {
        self.tx_submission_params = params;
        self
    }

    pub fn with_blockfetch_pipeline_n(mut self, n: NonZeroU8) -> Self {
        self.blockfetch_pipeline_n = n;
        self
    }

    pub fn with_max_n2n_version(mut self, version: VersionNumber) -> Self {
        self.max_n2n_version = version;
        self
    }

    pub fn with_max_inbound(mut self, max_inbound: usize) -> Self {
        self.max_inbound = max_inbound;
        self
    }

    pub fn with_share_request_delays(mut self, initial: Duration, interval: Duration) -> Self {
        self.share_request_initial_delay = initial;
        self.share_request_interval = interval;
        self
    }
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            connection_timeout: Duration::from_secs(2),
            reconnect_delay: Duration::from_secs(2),
            accept_interval: Duration::from_millis(100),
            tx_submission_params: ResponderParams::default(),
            blockfetch_pipeline_n: NonZeroU8::MIN,
            diffusion_stop_timeout: Duration::from_secs(300),
            maintenance_stop_timeout: Duration::from_secs(120),
            max_n2n_version: VersionNumber::CURRENT,
            max_inbound: DEFAULT_MAX_INBOUND,
            share_request_initial_delay: SHARE_REQUEST_INITIAL_DELAY,
            share_request_interval: SHARE_REQUEST_INTERVAL,
            share_request_amount: SHARE_REQUEST_AMOUNT,
        }
    }
}

impl Manager {
    async fn add_peer(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let already_dialing = matches!(
            self.peers.get(&peer).map(|state| &state.outbound),
            Some(OutboundState::Connected { .. } | OutboundState::Scheduled { .. })
        );
        if already_dialing {
            info!(protocols::manager::peer::CONNECT_DISCARDED, peer, reason = "already_connected_or_scheduled");
            return;
        }
        let attempt = self.next_attempt;
        self.next_attempt = self.next_attempt.wrapping_add(1);
        info!(protocols::manager::peer::CONNECT, peer);
        self.peers.entry(peer).or_default().outbound = OutboundState::Scheduled { attempt, conn_id: None };
        eff.ensure_child(&mut self.connector, "connector", connector::stage, || {
            connector::Connector::new(self.config.connection_timeout, eff.me())
        })
        .await;
        eff.send(&self.connector, connector::ConnectorMsg::Connect { peer, attempt }).await;
    }

    /// The dial `AddPeer` still has outstanding, before its socket id is known.
    fn current_attempt(&self, peer: Peer) -> Option<u64> {
        match self.peers.get(&peer).map(|state| &state.outbound) {
            Some(OutboundState::Scheduled { attempt, conn_id: None }) => Some(*attempt),
            _ => None,
        }
    }

    /// Report a failed attempt and forget that dial. The peer stays when an inbound bearer holds it.
    async fn abandon_attempt(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let has_inbound = self.peers.get(&peer).is_some_and(|state| state.inbound.is_some());
        if has_inbound {
            if let Some(state) = self.peers.get_mut(&peer) {
                state.outbound = OutboundState::None;
            }
        } else {
            self.peers.remove(&peer);
        }
        record_connect_failed(eff, peer).await;
    }

    async fn connection_result(
        &mut self,
        peer: Peer,
        attempt: u64,
        result: Result<ConnectionId, ConnectError>,
        eff: &Effects<ManagerMessage>,
    ) {
        match result {
            Ok(conn_id) => self.connection_succeeded(peer, attempt, conn_id, eff).await,
            Err(err) => {
                if self.current_attempt(peer) != Some(attempt) {
                    return;
                }
                info!(protocols::manager::peer::CONNECT_FAILED, peer, error = err.to_string());
                self.abandon_attempt(peer, eff).await;
            }
        }
    }

    async fn connection_succeeded(
        &mut self,
        peer: Peer,
        attempt: u64,
        conn_id: ConnectionId,
        eff: &Effects<ManagerMessage>,
    ) {
        if self.outstanding(conn_id) {
            return;
        }
        let bind = self.current_attempt(peer) == Some(attempt);
        if !bind {
            debug!(
                protocols::manager::peer::DISCONNECT_IGNORED,
                peer,
                reason = "rejected_duplicate",
                conn_id = conn_id.as_u64()
            );
            close_connection(eff, &peer, conn_id).await;
            return;
        }
        if let Some(state) = self.peers.get_mut(&peer) {
            state.outbound = OutboundState::Scheduled { attempt, conn_id: Some(conn_id) };
        }
        info!(protocols::manager::peer::CONNECTED, peer, conn_id = conn_id.as_u64());
        self.start_connection_stage(eff, peer, conn_id, ConnectionDirection::Outbound).await;
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
        // Remember the id before the stage runs. Duplicate detection (at most one inbound per peer)
        // happens after handshake; extras are terminated then. The handshake itself always runs.
        if self.outstanding(conn_id) {
            return;
        }
        self.start_connection_stage(eff, peer, conn_id, ConnectionDirection::Inbound).await;
    }

    /// Start a stage to handle the connection lifecycle.
    ///
    /// The id is pending once the stage exists and before it is told to run.
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
        self.pending.insert(conn_id, PendingConn { peer, stage: connection.clone(), direction });
        eff.send(&connection, ConnectionMessage::Initialize).await;
    }

    fn outstanding(&self, conn_id: ConnectionId) -> bool {
        self.pending.contains_key(&conn_id) || self.connections.contains_key(&conn_id)
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
        let Some(pending) = self.pending.remove(&conn_id) else {
            self.ignore_unknown(peer, conn_id);
            return;
        };
        if pending.peer != peer || pending.direction != direction_of(role) {
            self.pending.insert(conn_id, pending);
            self.ignore_unknown(peer, conn_id);
            return;
        }
        let direction = pending.direction;
        info!(
            protocols::manager::peer::HANDSHAKE_COMPLETED,
            peer,
            conn_id = conn_id.as_u64(),
            full_duplex_capable,
            full_duplex,
            advertisable
        );
        let inbound_live = self.peers.values().filter(|state| state.inbound.is_some()).count();
        let over_cap = direction == ConnectionDirection::Inbound
            && self.peers.get(&peer).is_none_or(|state| state.inbound.is_none())
            && inbound_live >= self.config.max_inbound;
        if over_cap {
            info!(protocols::manager::peer::INBOUND_REFUSED, peer, conn_id = conn_id.as_u64());
            let empty = self
                .peers
                .get(&peer)
                .is_none_or(|state| state.inbound.is_none() && state.outbound == OutboundState::None);
            if empty {
                self.peers.remove(&peer);
            }
            self.refuse_handshake(peer, conn_id, pending.stage, stage, eff).await;
            return;
        }
        let accept_this = self.take_direction_slot(peer, conn_id, direction);
        if accept_this {
            // Outbound handshake records Diffusion on the bearer, so a later fetch sees it.
            self.connections.insert(conn_id, Connection { stage, direction, full_duplex_capable, peer });
            record_established(eff, peer, conn_id, direction, full_duplex_capable, full_duplex, advertisable).await;
        } else {
            info!(protocols::manager::peer::DUPLICATE_TERMINATED, peer, conn_id = conn_id.as_u64());
            self.refuse_handshake(peer, conn_id, pending.stage, stage, eff).await;
        }
    }

    /// Claim the one slot for this direction. A slot already held by another id is not taken.
    fn take_direction_slot(&mut self, peer: Peer, conn_id: ConnectionId, direction: ConnectionDirection) -> bool {
        let state = self.peers.entry(peer).or_default();
        match direction {
            ConnectionDirection::Outbound => {
                let bound_here = match &state.outbound {
                    OutboundState::Scheduled { conn_id: Some(bound), .. } => *bound == conn_id,
                    OutboundState::None
                    | OutboundState::Scheduled { conn_id: None, .. }
                    | OutboundState::Connected { .. } => false,
                };
                if bound_here {
                    state.outbound = OutboundState::Connected { conn_id };
                }
                bound_here
            }
            ConnectionDirection::Inbound => {
                if state.inbound.is_some() {
                    false
                } else {
                    state.inbound = Some(conn_id);
                    true
                }
            }
        }
    }

    async fn refuse_handshake(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        pending_stage: StageRef<ConnectionMessage>,
        reported: StageRef<ConnectionMessage>,
        eff: &Effects<ManagerMessage>,
    ) {
        close_connection(eff, &peer, conn_id).await;
        eff.send(&pending_stage, ConnectionMessage::Disconnect).await;
        if pending_stage != reported {
            eff.send(&reported, ConnectionMessage::Disconnect).await;
        }
    }

    async fn remove_peer(&mut self, peer: Peer, eff: &Effects<ManagerMessage>) {
        let entry = self.peers.remove(&peer);
        let pending_ids: Vec<ConnectionId> =
            self.pending.iter().filter(|(_, pending)| pending.peer == peer).map(|(id, _)| *id).collect();
        if entry.is_none() && pending_ids.is_empty() {
            info!(protocols::manager::peer::DISCONNECT_IGNORED, peer, reason = "not_connected");
            return;
        }
        for conn_id in pending_ids {
            let Some(pending) = self.pending.remove(&conn_id) else { continue };
            let direction = match pending.direction {
                ConnectionDirection::Inbound => "inbound",
                ConnectionDirection::Outbound => "outbound",
            };
            info!(protocols::manager::peer::DISCONNECTING, peer, conn_id = conn_id.as_u64(), direction);
            close_connection(eff, &peer, conn_id).await;
            eff.send(&pending.stage, ConnectionMessage::Disconnect).await;
        }
        let Some(entry) = entry else { return };
        if let Some(conn_id) = entry.inbound {
            self.disconnect_live(peer, conn_id, "inbound", eff).await;
        }
        if let OutboundState::Connected { conn_id } = entry.outbound {
            self.disconnect_live(peer, conn_id, "outbound", eff).await;
        }
    }

    async fn disconnect_live(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        direction: &'static str,
        eff: &Effects<ManagerMessage>,
    ) {
        let Some(connection) = self.connections.remove(&conn_id) else { return };
        info!(protocols::manager::peer::DISCONNECTING, peer, conn_id = conn_id.as_u64(), direction);
        close_connection(eff, &peer, conn_id).await;
        record_closed(eff, peer, conn_id, CloseReason::LocalDisconnect).await;
        eff.send(&connection.stage, ConnectionMessage::Disconnect).await;
    }

    async fn connection_died(&mut self, peer: Peer, conn_id: ConnectionId, role: Role, eff: &Effects<ManagerMessage>) {
        // The stage has ended. The socket may already have been closed before it was asked to stop.
        close_terminated_connection(eff, &peer, conn_id).await;
        if let Some(pending) = self.pending.remove(&conn_id) {
            self.pending_died(peer, conn_id, pending, eff).await;
            return;
        }
        if let Some(connection) = self.connections.remove(&conn_id) {
            self.live_died(peer, conn_id, connection, role, eff).await;
            return;
        }
        self.ignore_unknown(peer, conn_id);
    }

    async fn pending_died(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        pending: PendingConn,
        eff: &Effects<ManagerMessage>,
    ) {
        debug!(
            protocols::manager::peer::DISCONNECT_IGNORED,
            peer,
            reason = "before_handshake",
            conn_id = conn_id.as_u64()
        );
        let is_current = pending.direction == ConnectionDirection::Outbound
            && matches!(
                self.peers.get(&peer).map(|state| &state.outbound),
                Some(OutboundState::Scheduled { conn_id: Some(id), .. }) if *id == conn_id
            );
        if is_current {
            self.abandon_attempt(peer, eff).await;
        }
    }

    async fn live_died(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        connection: Connection,
        role: Role,
        eff: &Effects<ManagerMessage>,
    ) {
        let expected = match connection.direction {
            ConnectionDirection::Inbound => Role::Responder,
            ConnectionDirection::Outbound => Role::Initiator,
        };
        assert_eq!(role, expected);
        let (remove, outcome) = if let Some(state) = self.peers.get_mut(&peer) {
            match connection.direction {
                ConnectionDirection::Inbound => {
                    assert_eq!(state.inbound, Some(conn_id));
                    if state.outbound == OutboundState::None {
                        (true, "peer_removed")
                    } else {
                        state.inbound = None;
                        (false, "kept_for_outbound")
                    }
                }
                ConnectionDirection::Outbound => {
                    assert_eq!(state.outbound, OutboundState::Connected { conn_id });
                    if state.inbound.is_none() {
                        (true, "peer_removed")
                    } else {
                        state.outbound = OutboundState::None;
                        (false, "kept_for_inbound")
                    }
                }
            }
        } else {
            (false, "peer_removed")
        };
        if remove {
            self.peers.remove(&peer);
        }
        info!(protocols::manager::peer::CONNECTION_DIED_HANDLED, peer, outcome);
        record_closed(eff, peer, conn_id, CloseReason::BearerEnded).await;
    }

    fn ignore_unknown(&self, peer: Peer, conn_id: ConnectionId) {
        let reason = if self.peers.contains_key(&peer) { "connection_not_found" } else { "peer_already_removed" };
        debug!(protocols::manager::peer::DISCONNECT_IGNORED, peer, reason, conn_id = conn_id.as_u64());
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
        let initiating = PeerTrack::new(eff).may_initiate().await;
        let mut contacted = Vec::new();
        match peers {
            None => {
                for (conn_id, conn) in &self.connections {
                    if !initiating.contains(conn_id) {
                        continue;
                    }
                    contacted.push(conn.peer);
                    eff.send(&conn.stage, ConnectionMessage::FetchBlocks { from, through, cr: cr.clone(), id }).await;
                }
            }
            Some(wanted) => {
                for peer in wanted {
                    let Some((_, conn)) = self
                        .connections
                        .iter()
                        .find(|(conn_id, conn)| initiating.contains(conn_id) && conn.peer == peer)
                    else {
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
}

/// The manager stage is responsible for managing the connections to the peers.
///
/// The semantics of the operations are as follows:
/// - AddPeer: add a peer to the manager unless that peer is already added
/// - RemovePeer: remove a peer from the manager, which terminates a live bearer or an in-flight
///   stage whose connection id is already known
///
/// A peer can be added right after being removed. A result for the dial that was cleared does not
/// attach to the new one. The old socket is closed when that dial is dropped.
pub async fn stage(mut manager: Manager, msg: ManagerMessage, eff: Effects<ManagerMessage>) -> Manager {
    let message_type = msg.message_type().to_string();
    let span = debug_span!(protocols::manager::message::PROCESS, message_type);

    async move {
        match msg {
            ManagerMessage::AddPeer(peer) => {
                let span = debug_span!(protocols::manager::peer::ADD, peer);
                manager.add_peer(peer, &eff).instrument(span.into()).await;
            }
            ManagerMessage::Accepted(peer, conn_id) => {
                let span = debug_span!(protocols::manager::peer::ACCEPTED, peer, conn_id = conn_id.as_u64());
                manager.accepted(peer, conn_id, &eff).instrument(span.into()).await;
            }
            ManagerMessage::RemovePeer(peer) => {
                let span = debug_span!(protocols::manager::peer::REMOVE, peer);
                manager.remove_peer(peer, &eff).instrument(span.into()).await;
            }
            ManagerMessage::Disconnect(peer, conn_id) => {
                debug!(
                    protocols::manager::peer::DISCONNECTING,
                    peer,
                    conn_id = conn_id.as_u64(),
                    direction = "requested"
                );
                let stage = manager
                    .connections
                    .get(&conn_id)
                    .map(|connection| connection.stage.clone())
                    .or_else(|| manager.pending.get(&conn_id).map(|pending| pending.stage.clone()));
                if let Some(stage) = stage {
                    close_connection(&eff, &peer, conn_id).await;
                    eff.send(&stage, ConnectionMessage::Disconnect).await;
                } else {
                    manager.ignore_unknown(peer, conn_id);
                }
            }
            ManagerMessage::ConnectionDied(peer, conn_id, role) => {
                let span = debug_span!(
                    protocols::manager::peer::CONNECTION_DIED,
                    peer,
                    conn_id = conn_id.as_u64(),
                    role = role.to_string(),
                );
                manager.connection_died(peer, conn_id, role, &eff).instrument(span.into()).await;
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
            ManagerMessage::ConnectionResult { peer, attempt, result } => {
                manager.connection_result(peer, attempt, result, &eff).await;
            }
            ManagerMessage::SetLocalUse { peer, conn_id, local_use } => {
                let stage = manager
                    .connections
                    .get(&conn_id)
                    .map(|connection| connection.stage.clone())
                    .or_else(|| manager.pending.get(&conn_id).map(|pending| pending.stage.clone()));
                if let Some(stage) = stage {
                    info!(
                        protocols::manager::peer::SET_LOCAL_USE,
                        peer,
                        conn_id = conn_id.as_u64(),
                        local_use = format!("{local_use:?}")
                    );
                    eff.send(&stage, ConnectionMessage::SetLocalUse(local_use)).await;
                }
            }
        }
        manager
    }
    .instrument(span.into())
    .await
}

fn direction_of(role: Role) -> ConnectionDirection {
    match role {
        Role::Initiator => ConnectionDirection::Outbound,
        Role::Responder => ConnectionDirection::Inbound,
    }
}

/// Local use stored at handshake: outbound starts in diffusion, inbound stays none.
fn handshake_local_use(direction: ConnectionDirection) -> LocalUse {
    match direction {
        ConnectionDirection::Outbound => LocalUse::Diffusion,
        ConnectionDirection::Inbound => LocalUse::None,
    }
}

async fn record_established(
    eff: &Effects<ManagerMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    direction: ConnectionDirection,
    full_duplex_capable: bool,
    full_duplex: bool,
    advertisable: bool,
) {
    let at = eff.clock().await;
    let conn = ConnectionRecord {
        peer,
        conn_id,
        direction,
        full_duplex_capable,
        full_duplex,
        advertisable,
        local_use: handshake_local_use(direction),
        established_at: observed_at(at),
    };
    PeerTrack::new(eff).record_connection_established(conn, at).await;
}

async fn record_closed(eff: &Effects<ManagerMessage>, peer: Peer, conn_id: ConnectionId, reason: CloseReason) {
    let at = eff.clock().await;
    PeerTrack::new(eff).record_connection_closed(peer, conn_id, reason, at).await;
}

async fn record_connect_failed(eff: &Effects<ManagerMessage>, peer: Peer) {
    let at = eff.clock().await;
    PeerTrack::new(eff).record_connect_failed(peer, at).await;
}

/// Close the socket before the connection stage is asked to stop.
async fn close_connection(eff: &Effects<ManagerMessage>, peer: &Peer, conn_id: ConnectionId) {
    if let Err(err) = Network::new(eff).close(conn_id).await {
        error!(protocols::manager::peer::CLOSE_FAILED, peer, error = err.to_string());
    }
}

/// Close the socket after the connection stage has terminated.
///
/// The socket may already have been closed. That error is logged at debug.
async fn close_terminated_connection(eff: &Effects<ManagerMessage>, peer: &Peer, conn_id: ConnectionId) {
    if let Err(err) = Network::new(eff).close(conn_id).await {
        debug!(protocols::manager::peer::CLOSE_FAILED, peer, error = err.to_string());
    }
}

pub fn register_deserializers() -> DeserializerGuards {
    let mut guards = vec![
        register_data_deserializer::<Manager>().boxed(),
        register_data_deserializer::<ManagerMessage>().boxed(),
        register_data_deserializer::<Instant>().boxed(),
    ];
    guards.extend(connector::register_deserializers());
    guards
}
