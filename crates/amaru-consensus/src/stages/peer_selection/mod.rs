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

use amaru_kernel::{BlockHeight, Peer};
use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::manager::ManagerMessage;
use pure_stage::{Effects, ScheduleId, StageRef};
use rand::seq::IteratorRandom;

use crate::effects::{Ledger, LedgerOps};

const STATIC_PEER_BAN_PERIOD: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerSelection {
    target_upstream_peers: usize,
    target_downstream_peers: usize,
    manager: StageRef<ManagerMessage>,
    static_peers: BTreeSet<Peer>,
    ledger_candidates: BTreeSet<Peer>,
    peer_removal_cooldown: Duration,
    cooldown_timers: BTreeMap<Peer, ScheduleId>,
    inbound_peers: BTreeMap<Peer, PeerState>,
    outbound_peers: BTreeMap<Peer, PeerState>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum PeerState {
    Connecting,
    Connected(Connection),
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    id: ConnectionId,
    full_duplex_capable: bool,
    full_duplex: bool,
}

impl Connection {
    pub fn new(id: ConnectionId, full_duplex_capable: bool, full_duplex: bool) -> Self {
        Self { id, full_duplex_capable, full_duplex }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSelectionMsg {
    /// This message is required to be sent to the peer_selection stage at startup.
    ///
    /// This will connect to initial peers and start the ledger check.
    Initialize,
    /// The peer has performed an adversarial action, such as sending invalid blocks or headers.
    ///
    /// This peer will be removed and banned for some time period; static peers are banned
    /// shorter than non-static peers.
    Adversarial(Peer),
    /// Manually add a peer, mostly for testing.
    AddPeer(Peer),
    /// The cooldown period for a peer has ended, and the peer can be re-added.
    CooldownEnded(Peer),
    /// A peer has connected and the peer_selection stage can start tracking it.
    ///
    /// This may be a downstream peer or the successful result of a connection attempt.
    Connected(Peer, Connection, ConnectionDirection),
    /// A peer has disconnected and the peer_selection stage can stop tracking it.
    ///
    /// This is also the message sent when a connection attempt fails.
    Disconnected(Peer, ConnectionDirection),
    /// Internal message from ledger check with new candidates.
    LedgerCheckCandidates(BTreeSet<Peer>),
}

impl PeerSelection {
    pub fn new(
        manager: StageRef<ManagerMessage>,
        static_peers: BTreeSet<Peer>,
        target_upstream_peers: usize,
        target_downstream_peers: usize,
        peer_removal_cooldown_secs: u64,
    ) -> Self {
        Self {
            target_upstream_peers,
            target_downstream_peers,
            ledger_candidates: BTreeSet::new(),
            manager,
            static_peers,
            peer_removal_cooldown: Duration::from_secs(peer_removal_cooldown_secs),
            cooldown_timers: BTreeMap::new(),
            inbound_peers: BTreeMap::new(),
            outbound_peers: BTreeMap::new(),
        }
    }
}

pub async fn stage(mut state: PeerSelection, msg: PeerSelectionMsg, eff: Effects<PeerSelectionMsg>) -> PeerSelection {
    match msg {
        PeerSelectionMsg::Initialize => {
            tracing::info!(peers = state.static_peers.len(), "peer_selection.connect_initial");
            for p in &state.static_peers {
                eff.send(&state.manager, ManagerMessage::AddPeer(p.clone())).await;
                state.outbound_peers.insert(p.clone(), PeerState::Connecting);
            }
            let ledger_check = eff
                .wire_up(
                    eff.stage("peer-selection/ledger-check", get_ledger_candidates).await,
                    LedgerCheck::new(eff.me()),
                )
                .await;
            eff.send(&ledger_check, ()).await;
        }
        PeerSelectionMsg::Adversarial(peer) => {
            tracing::debug!(%peer, "peer_selection.adversarial");
            ban_peer(&mut state, peer, eff).await;
        }
        PeerSelectionMsg::CooldownEnded(peer) => {
            state.cooldown_timers.remove(&peer);
        }
        PeerSelectionMsg::AddPeer(peer) => {
            tracing::info!(%peer, "peer_selection.add_peer");
            if state.cooldown_timers.contains_key(&peer) {
                tracing::info!(%peer, "peer is banned");
            } else {
                eff.send(&state.manager, ManagerMessage::AddPeer(peer.clone())).await;
                state.outbound_peers.insert(peer, PeerState::Connecting);
            }
        }
        PeerSelectionMsg::Connected(peer, connection, ConnectionDirection::Inbound) => {
            if state.inbound_peers.len() >= state.target_downstream_peers {
                tracing::info!(%peer, "inbound peer limit reached");
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, connection.id)).await;
                return state;
            }
            let old = state.inbound_peers.insert(peer.clone(), PeerState::Connected(connection));
            if let Some(PeerState::Connected(conn)) = old {
                tracing::info!(%peer, ?conn, "inbound connection replaced by peer");
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, conn.id)).await;
            }
        }
        PeerSelectionMsg::Connected(peer, connection, ConnectionDirection::Outbound) => {
            let old = state.outbound_peers.insert(peer.clone(), PeerState::Connected(connection));
            if let Some(PeerState::Connected(conn)) = old {
                tracing::warn!(%peer, ?conn, "connected outbound while still connected");
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, conn.id)).await;
            }
        }
        PeerSelectionMsg::Disconnected(peer, ConnectionDirection::Inbound) => {
            state.inbound_peers.remove(&peer);
        }
        PeerSelectionMsg::Disconnected(peer, ConnectionDirection::Outbound) => {
            let old = state.outbound_peers.remove(&peer);
            if old == Some(PeerState::Connecting) {
                cool_down(&mut state, peer, &eff, false).await;
            }
            regulate_peers(&mut state, eff).await;
        }
        PeerSelectionMsg::LedgerCheckCandidates(candidates) => {
            state.ledger_candidates = candidates;
            regulate_peers(&mut state, eff).await;
        }
    }
    state
}

async fn ban_peer(state: &mut PeerSelection, peer: Peer, eff: Effects<PeerSelectionMsg>) {
    let is_static = state.static_peers.contains(&peer);

    let mut send_remove = false;
    if let Some(peer_state) = state.inbound_peers.remove(&peer) {
        tracing::warn!(%peer, ?peer_state, is_static, "removing peer (inbound)");
        send_remove = true;
    }

    if let Some(peer_state) = state.outbound_peers.remove(&peer) {
        tracing::warn!(%peer, ?peer_state, is_static, "removing peer (outbound)");
        send_remove = true;
    }

    if send_remove {
        eff.send(&state.manager, ManagerMessage::RemovePeer(peer.clone())).await;
    }

    cool_down(state, peer, &eff, is_static).await;
}

async fn cool_down(state: &mut PeerSelection, peer: Peer, eff: &Effects<PeerSelectionMsg>, is_static: bool) {
    let ban_period = if is_static { STATIC_PEER_BAN_PERIOD } else { state.peer_removal_cooldown };
    let id = eff.schedule_after(PeerSelectionMsg::CooldownEnded(peer.clone()), ban_period).await;
    let old = state.cooldown_timers.insert(peer, id);
    if let Some(id) = old {
        eff.cancel_schedule(id).await;
    }
}

async fn regulate_peers(state: &mut PeerSelection, eff: Effects<PeerSelectionMsg>) {
    let target_upstream_peers = state.target_upstream_peers as usize;
    let outbound_peers = state.outbound_peers.len();
    if outbound_peers < target_upstream_peers {
        let candidates = state
            .ledger_candidates
            .iter()
            .filter(|p| !state.outbound_peers.contains_key(p) && !state.cooldown_timers.contains_key(p))
            .cloned()
            .choose_multiple(&mut rand::rng(), target_upstream_peers - outbound_peers);
        for peer in candidates {
            eff.send(&state.manager, ManagerMessage::AddPeer(peer.clone())).await;
            state.outbound_peers.insert(peer, PeerState::Connecting);
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct LedgerCheck {
    last_height: BlockHeight,
    cadence: Duration,
    min_height_change: u64,
    stage: StageRef<PeerSelectionMsg>,
}

impl LedgerCheck {
    fn new(stage: StageRef<PeerSelectionMsg>) -> Self {
        Self { last_height: BlockHeight::from(0), cadence: Duration::from_secs(60), min_height_change: 3000, stage }
    }
}

#[tracing::instrument(level = "info", skip_all, fields(last_height = %state.last_height))]
async fn get_ledger_candidates(mut state: LedgerCheck, _msg: (), eff: Effects<()>) -> LedgerCheck {
    let ledger = Ledger::new(eff.clone());
    let current_height = ledger.volatile_tip().unwrap_or_else(|| ledger.tip()).block_height();
    if current_height < state.last_height + state.min_height_change {
        return reschedule_check(state, eff).await;
    }
    let ledger_entries = ledger.registered_relay_socket_addrs().await;
    let ledger_entries = match ledger_entries {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(%e, "failed to get ledger entries");
            return reschedule_check(state, eff).await;
        }
    };
    let ledger_entries = ledger_entries.into_iter().map(|entry| Peer::from_addr(&entry)).collect();
    eff.send(&state.stage, PeerSelectionMsg::LedgerCheckCandidates(ledger_entries)).await;
    state.last_height = current_height;
    reschedule_check(state, eff).await
}

async fn reschedule_check(state: LedgerCheck, eff: Effects<()>) -> LedgerCheck {
    eff.schedule_after((), state.cadence).await;
    state
}

// #[cfg(test)]
// mod test_setup;
// #[cfg(test)]
// mod tests;
