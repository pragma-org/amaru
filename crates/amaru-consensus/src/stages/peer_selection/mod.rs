// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use amaru_kernel::Peer;
use amaru_protocols::manager::ManagerMessage;
use pure_stage::{Effects, ScheduleId, StageRef};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerSelection {
    manager: StageRef<ManagerMessage>,
    static_peers: BTreeSet<Peer>,
    peer_removal_cooldown: Duration,
    cooldown_timers: BTreeMap<Peer, ScheduleId>,
    downstream_connected: BTreeSet<Peer>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSelectionMsg {
    RemovePeer(Peer),
    AddPeer(Peer),
    ConnectInitial,
    CooldownEnded(Peer),
    DownstreamConnected(Peer),
}

impl PeerSelection {
    pub fn new(
        manager: StageRef<ManagerMessage>,
        static_peers: BTreeSet<Peer>,
        peer_removal_cooldown_secs: u64,
    ) -> Self {
        Self {
            manager,
            static_peers,
            peer_removal_cooldown: Duration::from_secs(peer_removal_cooldown_secs),
            cooldown_timers: BTreeMap::new(),
            downstream_connected: BTreeSet::new(),
        }
    }
}

pub async fn stage(mut state: PeerSelection, msg: PeerSelectionMsg, eff: Effects<PeerSelectionMsg>) -> PeerSelection {
    match msg {
        PeerSelectionMsg::ConnectInitial => {
            tracing::info!(peers = state.static_peers.len(), "peer_selection.connect_initial");
            for p in &state.static_peers {
                eff.send(&state.manager, ManagerMessage::AddPeer(p.clone())).await;
            }
        }
        PeerSelectionMsg::RemovePeer(peer) => {
            tracing::info!(%peer, "peer_selection.remove_peer");
            if state.static_peers.contains(&peer) {
                tracing::warn!(%peer, "removing static peer");
            }
            if let Some(id) = state.cooldown_timers.remove(&peer) {
                eff.cancel_schedule(id).await;
            }
            eff.send(&state.manager, ManagerMessage::RemovePeer(peer.clone())).await;
            let id =
                eff.schedule_after(PeerSelectionMsg::CooldownEnded(peer.clone()), state.peer_removal_cooldown).await;
            state.cooldown_timers.insert(peer, id);
        }
        PeerSelectionMsg::CooldownEnded(peer) => {
            if state.cooldown_timers.remove(&peer).is_none() {
                return state;
            }
            if state.static_peers.contains(&peer) {
                tracing::info!(%peer, "peer_selection.reconnect_static_peer");
                eff.send(&state.manager, ManagerMessage::AddPeer(peer)).await;
            }
        }
        PeerSelectionMsg::AddPeer(peer) => {
            tracing::info!(%peer, "peer_selection.add_peer");
            if state.cooldown_timers.contains_key(&peer) {
                return state;
            }
            eff.send(&state.manager, ManagerMessage::AddPeer(peer)).await;
        }
        PeerSelectionMsg::DownstreamConnected(peer) => {
            if state.downstream_connected.insert(peer.clone()) {
                tracing::info!(%peer, "peer_selection.downstream_connected");
            }
        }
    }
    state
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
