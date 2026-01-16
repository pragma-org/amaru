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
use amaru_kernel::{Point, peer::Peer, protocol_messages::network_magic::NetworkMagic};
use amaru_ouroboros::{ConnectionId, ToSocketAddrs};
use pure_stage::{Effects, StageRef};
use std::{collections::BTreeMap, time::Duration};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManagerMessage {
    AddPeer(Peer),
    /// Internal message sent from the connection stage only!
    ConnectionDied(Peer, ConnectionId),
    Connect(Peer),
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
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum ConnectionState {
    Scheduled,
    Connected(ConnectionId, StageRef<ConnectionMessage>),
    Disconnecting(ConnectionId),
}

impl Manager {
    pub fn new(magic: NetworkMagic, chain_sync: StageRef<ChainSyncInitiatorMsg>) -> Self {
        Self {
            peers: BTreeMap::new(),
            magic,
            chain_sync,
        }
    }
}

pub async fn stage(
    mut manager: Manager,
    msg: ManagerMessage,
    eff: Effects<ManagerMessage>,
) -> Manager {
    match msg {
        ManagerMessage::AddPeer(peer) => {
            if manager.peers.contains_key(&peer) {
                tracing::info!(%peer, "peer already added");
                return manager;
            }
            tracing::info!(%peer, "adding peer");
            manager
                .peers
                .insert(peer.clone(), ConnectionState::Scheduled);
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
                Some(ConnectionState::Disconnecting(..)) => {
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
            let connection = eff
                .stage(format!("{conn_id}-{peer}"), connection::stage)
                .await;
            let connection = eff.supervise(
                connection,
                ManagerMessage::ConnectionDied(peer.clone(), conn_id),
            );
            let connection = eff
                .wire_up(
                    connection,
                    connection::Connection::new(
                        peer.clone(),
                        conn_id,
                        Role::Initiator,
                        manager.magic,
                        manager.chain_sync.clone(),
                    ),
                )
                .await;
            eff.send(&connection, ConnectionMessage::Initialize).await;
            *entry = ConnectionState::Connected(conn_id, connection);
        }
        ManagerMessage::RemovePeer(peer) => {
            let Some(entry) = manager.peers.get_mut(&peer) else {
                tracing::info!(%peer, "disconnect request ignored, not connected");
                return manager;
            };
            match entry {
                ConnectionState::Connected(conn_id, connection) => {
                    eff.send(connection, ConnectionMessage::Disconnect).await;
                    *entry = ConnectionState::Disconnecting(*conn_id);
                }
                ConnectionState::Scheduled | ConnectionState::Disconnecting(..) => {
                    tracing::info!(%peer, "removing currently disconnected peer");
                    manager.peers.remove(&peer);
                }
            }
        }
        ManagerMessage::ConnectionDied(peer, conn_id) => {
            if let Err(err) = Network::new(&eff).close(conn_id).await {
                tracing::error!(?err, %peer, "failed to close connection");
            }
            let Some(peer_state) = manager.peers.get_mut(&peer) else {
                tracing::debug!(%peer, "connection died, peer already removed");
                return manager;
            };
            match peer_state {
                ConnectionState::Connected(..) => {
                    tracing::info!(%peer, "disconnected from peer, scheduling reconnect");
                    eff.schedule_after(ManagerMessage::Connect(peer), Duration::from_secs(10))
                        .await;
                    *peer_state = ConnectionState::Scheduled;
                }
                ConnectionState::Scheduled => {
                    tracing::debug!(%peer, "connection died, peer already scheduled");
                    return manager;
                }
                ConnectionState::Disconnecting(..) => {
                    tracing::debug!(%peer, "peer terminated after removal");
                    manager.peers.remove(&peer);
                    return manager;
                }
            }
        }
        ManagerMessage::FetchBlocks {
            peer,
            from,
            through,
            cr,
        } => {
            tracing::info!(?from, ?through, %peer, "fetching blocks");
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
