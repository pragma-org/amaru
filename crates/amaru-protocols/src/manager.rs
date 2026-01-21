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
    blockfetch::Blocks,
    chainsync::ChainSyncInitiatorMsg,
    connection::{self, ConnectionMessage},
    network_effects::{Network, NetworkOps},
    protocol::Role,
};
use amaru_kernel::{NetworkMagic, Peer, Point};
use amaru_ouroboros::{ConnectionId, ToSocketAddrs};
use pure_stage::{Effects, StageRef};
use std::sync::Arc;
use std::{collections::BTreeMap, time::Duration};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManagerMessage {
    AddPeer(Peer),
    /// Internal message sent from the connection stage only!
    ///
    /// Must contain the connection ID so that we can then close the actual socket;
    /// the `peers` entry could already have been removed by RemovePeer.
    // TODO move to separate message type
    ConnectionDied(Peer, ConnectionId, Role),
    // TODO move to separate message type
    Connect(Peer),
    Accepted(Peer, ConnectionId),
    RemovePeer(Peer),
    FetchBlocks {
        peer: Peer,
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
    },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Manager {
    peers: BTreeMap<Peer, ConnectionState>,
    magic: NetworkMagic,
    chain_sync: StageRef<ChainSyncInitiatorMsg>,
    era_history: Arc<amaru_slot_arithmetic::EraHistory>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum ConnectionState {
    Scheduled,
    Connected(ConnectionId, StageRef<ConnectionMessage>),
    // Does not contain the connection ID because that will be received in the ConnectionDied message.
    Disconnecting,
}

impl Manager {
    pub fn new(
        magic: NetworkMagic,
        chain_sync: StageRef<ChainSyncInitiatorMsg>,
        era_history: Arc<amaru_slot_arithmetic::EraHistory>,
    ) -> Self {
        Self {
            peers: BTreeMap::new(),
            magic,
            chain_sync,
            era_history: era_history.clone(),
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
pub async fn stage(
    mut manager: Manager,
    msg: ManagerMessage,
    eff: Effects<ManagerMessage>,
) -> Manager {
    match msg {
        ManagerMessage::AddPeer(peer) => {
            match manager.peers.get_mut(&peer) {
                Some(ConnectionState::Connected(..) | ConnectionState::Scheduled) => {
                    tracing::info!(%peer, "discarding connection request, already connected or scheduled");
                    return manager;
                }
                Some(s @ ConnectionState::Disconnecting) => {
                    tracing::info!(%peer, "adding peer while still disconnecting");
                    // old connection stage will report ConnectionDied which will close the socket
                    *s = ConnectionState::Scheduled;
                }
                None => {
                    tracing::info!(%peer, "adding peer");
                    manager
                        .peers
                        .insert(peer.clone(), ConnectionState::Scheduled);
                }
            }
            eff.send(eff.me_ref(), ManagerMessage::Connect(peer)).await;
        }
        ManagerMessage::Connect(peer) => {
            // TODO(network) slow connection will block the manager, should delegate to a child stage
            let entry = match manager.peers.get_mut(&peer) {
                Some(ConnectionState::Connected(..)) => {
                    tracing::debug!(%peer, "discarding connection request, already connected");
                    return manager;
                }
                Some(entry @ ConnectionState::Scheduled) => entry,
                Some(ConnectionState::Disconnecting) => {
                    tracing::debug!(%peer, "discarding connection request, already disconnecting");
                    return manager;
                }
                None => {
                    tracing::debug!(%peer, "discarding connection request, not added");
                    return manager;
                }
            };
            let addr = ToSocketAddrs::String(peer.to_string());
            let conn_id = match Network::new(&eff)
                .connect(addr, Duration::from_secs(10))
                .await
            {
                Ok(conn_id) => conn_id,
                Err(err) => {
                    tracing::error!(?err, %peer, "failed to connect to peer");
                    eff.schedule_after(ManagerMessage::Connect(peer), Duration::from_secs(10))
                        .await;
                    assert_eq!(*entry, ConnectionState::Scheduled);
                    return manager;
                }
            };
            tracing::info!(?conn_id, %peer, "connected to peer");
            start_connection_stage(&mut manager, &eff, peer, conn_id, Role::Initiator).await;
        }
        ManagerMessage::Accepted(peer, conn_id) => {
            match manager.peers.get(&peer) {
                Some(ConnectionState::Connected(..)) => {
                    tracing::debug!(%peer, "already connected. Closing the newly accepted connection");
                    close_connection(&eff, &peer, conn_id).await;
                    return manager;
                }
                Some(ConnectionState::Disconnecting) => {
                    tracing::debug!(%peer, "already disconnecting, the previous connection will be closed with ConnectionDied, the newly accepted connection will be closed now");
                    close_connection(&eff, &peer, conn_id).await;
                    return manager;
                }
                Some(ConnectionState::Scheduled) => {
                    unreachable!(
                        "Accepted peers are initiators. They will schedule reconnections on their side so this case cannot happen."
                    )
                }
                None => {}
            };
            start_connection_stage(&mut manager, &eff, peer, conn_id, Role::Responder).await;
        }
        ManagerMessage::RemovePeer(peer) => {
            let Some(entry) = manager.peers.get_mut(&peer) else {
                tracing::info!(%peer, "disconnect request ignored, not connected");
                return manager;
            };
            match entry {
                ConnectionState::Connected(_conn_id, connection) => {
                    eff.send(connection, ConnectionMessage::Disconnect).await;
                    *entry = ConnectionState::Disconnecting;
                }
                ConnectionState::Scheduled | ConnectionState::Disconnecting => {
                    tracing::info!(%peer, "removing currently disconnected peer");
                    manager.peers.remove(&peer);
                }
            }
        }
        ManagerMessage::ConnectionDied(peer, conn_id, role) => {
            close_connection(&eff, &peer, conn_id).await;
            let Some(peer_state) = manager.peers.get_mut(&peer) else {
                tracing::debug!(%peer, "connection died, peer already removed");
                return manager;
            };
            match peer_state {
                ConnectionState::Connected(conn_id_new, ..) if *conn_id_new != conn_id => {
                    tracing::debug!(%peer, "previously terminated connection closed");
                }
                ConnectionState::Connected(..) => {
                    // Only reconnect on the initiator side
                    if role == Role::Initiator {
                        tracing::info!(%peer, "initiator connection died, scheduling reconnect");
                        eff.schedule_after(ManagerMessage::Connect(peer), Duration::from_secs(10))
                            .await;
                        *peer_state = ConnectionState::Scheduled;
                    } else {
                        tracing::info!(%peer, "responder connection died, removing peer");
                        manager.peers.remove(&peer);
                    }
                }
                ConnectionState::Scheduled => {
                    tracing::debug!(%peer, "initiator connection died, reconnect already scheduled");
                }
                ConnectionState::Disconnecting => {
                    tracing::debug!(%peer, "peer terminated after removal");
                    manager.peers.remove(&peer);
                }
            }
        }
        ManagerMessage::FetchBlocks {
            peer,
            from,
            through,
            cr,
        } => {
            tracing::trace!(?from, ?through, %peer, "fetching blocks");
            if let Some(ConnectionState::Connected(_, connection)) = manager.peers.get(&peer) {
                eff.send(
                    connection,
                    ConnectionMessage::FetchBlocks { from, through, cr },
                )
                .await;
            } else {
                tracing::error!(%peer, "peer not found");
                eff.send(&cr, Blocks::default()).await;
            }
        }
    }
    manager
}

/// Close the connection and log any errors.
async fn close_connection(eff: &Effects<ManagerMessage>, peer: &Peer, conn_id: ConnectionId) {
    if let Err(err) = Network::new(eff).close(conn_id).await {
        tracing::error!(?err, %peer, "failed to close connection");
    }
}

/// Start a stage to handle the connection lifecycle.
async fn start_connection_stage(
    manager: &mut Manager,
    eff: &Effects<ManagerMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    role: Role,
) {
    let connection = eff
        .stage(format!("{conn_id}-{peer}"), connection::stage)
        .await;
    let connection = eff.supervise(
        connection,
        ManagerMessage::ConnectionDied(peer.clone(), conn_id, role),
    );
    let connection = eff
        .wire_up(
            connection,
            connection::Connection::new(
                peer.clone(),
                conn_id,
                role,
                manager.magic,
                manager.chain_sync.clone(),
                manager.era_history.clone(),
            ),
        )
        .await;
    eff.send(&connection, ConnectionMessage::Initialize).await;
    manager
        .peers
        .insert(peer, ConnectionState::Connected(conn_id, connection));
}
