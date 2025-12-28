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
    network_effects::ConnectEffect,
    protocol::Role,
};
use amaru_kernel::{Point, peer::Peer, protocol_messages::network_magic::NetworkMagic};
use amaru_ouroboros::{ConnectionId, ToSocketAddrs};
use pure_stage::{CallRef, Effects, StageRef};
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManagerMessage {
    AddPeer(Peer),
    FetchBlocks {
        peer: Peer,
        from: Point,
        through: Point,
        cr: CallRef<Blocks>,
    },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Manager {
    peers: BTreeMap<Peer, (ConnectionId, StageRef<ConnectionMessage>)>,
    magic: NetworkMagic,
    chain_sync: StageRef<ChainSyncInitiatorMsg>,
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
            let addr = ToSocketAddrs::String(peer.to_string());
            let conn_id = match eff.external(ConnectEffect { addr }).await {
                Ok(conn_id) => conn_id,
                Err(err) => {
                    tracing::error!(?err, %peer, "failed to connect to peer");
                    return manager;
                }
            };
            tracing::info!(?conn_id, %peer, "connected to peer");
            let connection = eff
                .stage(format!("{conn_id}-{peer}"), connection::stage)
                .await;
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
            manager.peers.insert(peer, (conn_id, connection));
        }
        ManagerMessage::FetchBlocks {
            peer,
            from,
            through,
            cr,
        } => {
            tracing::info!(?from, ?through, %peer, "fetching blocks");
            let connection = manager.peers.get(&peer).unwrap();
            eff.send(
                &connection.1,
                ConnectionMessage::FetchBlocks { from, through, cr },
            )
            .await;
        }
    }
    manager
}
