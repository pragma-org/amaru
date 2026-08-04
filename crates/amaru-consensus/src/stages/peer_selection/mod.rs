// Copyright 2026 PRAGMA
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

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap, btree_map::Entry},
    time::Duration,
};

use amaru_kernel::{BlockHeight, Peer};
use amaru_observability::{TraceContext, debug_span};
use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::{Effects, Instant, ScheduleId, StageRef};
use rand::{SeedableRng, rngs::StdRng, seq::IteratorRandom};
use tracing::Instrument;

use crate::effects::{GenerateRandomSeed, Ledger, LedgerOps};

const STATIC_PEER_BAN_PERIOD: Duration = Duration::from_secs(10);

/// Peer selection stage for the Amaru consensus node.
///
/// This stage is responsible for maintaining the desired number of outbound (upstream)
/// and inbound (downstream) peer connections. It acts as the decision point that tells
/// the `Manager` (via `ManagerMessage`) which peers to `AddPeer` or `RemovePeer`/`Disconnect`,
/// while reacting to connection lifecycle events and adversarial signals.
///
/// It maintains three primary peer pools:
/// - `static_peers` (immutable, configured at construction; preferred for outbound).
/// - `snapshot_candidates` (immutable, from a ledger peer snapshot file at construction).
/// - `ledger_candidates` (dynamic BTreeSet, updated via the child ledger-check protocol).
///
/// Outbound regulation uses a random refill (via `GenerateRandomSeed` external effect)
/// preferring static peers, then snapshot candidates, then ledger candidates, while
/// skipping any peer currently tracked in `outbound_peers` or `cooldown_until`.
/// Inbound connections are accepted up to `target_downstream_peers` (excess are
/// immediately rejected with a `Disconnect`).
///
/// The stage creates (on `Initialize`, with no supervision) a child stage
/// `"peer-selection/ledger-check"` running `get_ledger_candidates` (backed by `LedgerCheck`
/// state) that periodically queries the ledger for registered relay addresses and
/// feeds candidates back via `LedgerCheckCandidates`.
///
/// ## State
///
/// - `target_upstream_peers`, `target_downstream_peers`: configuration targets.
/// - `manager`: `StageRef<ManagerMessage>` for all outbound commands.
/// - `static_peers`, `snapshot_candidates`, `ledger_candidates`: candidate pools (`BTreeSet<Peer>`).
/// - `peer_removal_cooldown`: duration for non-static bans.
/// - `cooldown_until`: `BTreeMap<Peer, Instant>` of active bans (end time per peer).
/// - `cooldown_heap`: min-heap of pending `(Instant, Peer)` cool-down entries
///   (may contain stale entries after re-ban or early `AddPeer` lift).
/// - `cooldown_timer`: at most one outstanding `schedule_at(CheckCooldowns)` for the
///   earliest heap entry (`None` when no cool-downs are pending).
/// - `inbound_peers`: `BTreeMap<Peer, Connection>` (downstream tracking).
/// - `outbound_peers`: `BTreeMap<Peer, PeerState>` (`Connecting` or `Connected(Connection)`).
///
/// ## Message Handling
///
/// All behaviour is implemented in the single `match msg` inside `pub async fn stage`.
/// The stage is purely message-driven; there are no background loops outside scheduled
/// messages and the child stage.
///
/// - **Initialize**: Required at startup. Logs
///   `"peer_selection.connect_initial"`. For every `static_peers` entry: sends
///   `ManagerMessage::AddPeer` and records it as `PeerState::Connecting` in
///   `outbound_peers`. Then calls `regulate_peers` so snapshot (and later ledger)
///   candidates can fill any remaining outbound deficit. Unconditionally wires a
///   new child stage `"peer-selection/ledger-check"` (via `eff.stage` + `eff.wire_up`
///   with `LedgerCheck::new(eff.me())`, no supervision) and sends `()` to kick it off;
///   "failure in ledger-check shall tear down the node".
///
/// - **Adversarial**: Debug-logs. Delegates to
///   `ban_peer`: removes the peer from `inbound_peers` (if present;
///   warns `"removing peer (inbound)"`) and/or `outbound_peers` (if present; warns
///   `"removing peer (outbound)"`, calls `regulate_peers`, marks for removal).
///   If any removal occurred, sends `ManagerMessage::RemovePeer` to the manager.
///   Always calls `cool_down` (which computes `STATIC_PEER_BAN_PERIOD` (10s) for
///   static peers vs. configured cooldown, pushes onto the cool-down min-heap, and
///   arms `eff.schedule_at(CheckCooldowns)` only when the live cool-down set goes from
///   empty to one item, or when the new end time is earlier than the currently armed timer).
///
/// - **AddPeer**: Manual/test hook. If the peer
///   has an active cool-down, removes it from `cooldown_until` (`was_banned = true`)
///   and cancels the armed timer if no cool-downs remain. If the peer is not already
///   in `outbound_peers`: logs `"peer_selection.add_peer"` (with `was_banned`), sends
///   `ManagerMessage::AddPeer`, and inserts as `Connecting`. Otherwise logs that it is
///   not adding.
///
/// - **CheckCooldowns**: Clears any armed timer (cancel is a no-op if this message was
///   the delivery), drains all due min-heap entries (removes a peer from `cooldown_until`
///   when its stored deadline is `<= now` — so a re-ban with a later deadline is kept),
///   calls `regulate_peers`, then arms the next earliest heap entry via `schedule_at`
///   (if any). A past `Instant` is delivered on the priority path as soon as the runtime
///   can run this stage again.
///
/// - **Connected**:
///   - `Inbound`: If `inbound_peers.len() >= target_downstream_peers`, logs
///     `"rejecting inbound connection: too many peers"`, sends `ManagerMessage::Disconnect`,
///     and returns early (no insert). Otherwise inserts (or replaces a prior
///     connection for the same peer, sending `Disconnect` for the old one).
///   - `Outbound`: Inserts/updates as `PeerState::Connected(conn)`. If replacing
///     a prior `Connected` state, warns and sends `Disconnect` for the old conn.
///     (Transitions `Connecting` → `Connected` from successful manager attempts.)
///
/// - **Disconnected**:
///   - `Inbound`: Removes from `inbound_peers` only on exact `ConnectionId` match
///     (via `Entry::Occupied` guard).
///   - `Outbound` + `will_retry == true`: No-op (early match guard; state unchanged,
///     no effects, no regulation, no cooldown).
///   - `Outbound` + `will_retry == false`: Removes only if present as exactly
///     `PeerState::Connected` with matching id; then calls `regulate_peers`.
///     (Connecting-state peers with `!will_retry` reach the arm but the inner
///     `let PeerState::Connected` guard fails silently.)
///
/// - **ConnectFailed**: Removes the peer from
///   `outbound_peers` (any `PeerState`), then calls `regulate_peers`.
///
/// - **LedgerCheckCandidates**:
///   Replaces `ledger_candidates` wholesale, then calls `regulate_peers`.
///
/// ## Helper Methods
///
/// - `ban_peer`: Core removal + ban logic (used only by `Adversarial`).
/// - `cool_down`: Computes ban end, pushes onto the min-heap, arms at most one timer.
/// - `regulate_peers`: Core outbound refill logic (see below).
///
/// ## Ledger-Check Child Protocol
///
/// `LedgerCheck` holds `last_height`, `cadence` (60s), `min_height_change` (3000),
/// and a `StageRef<PeerSelectionMsg>` back to the parent. The child fn
/// `get_ledger_candidates` (instrumented) is kicked with `()`:
/// - Uses `Ledger::new(eff.clone())` (from `crate::effects`).
/// - Queries `volatile_tip().block_height()`.
/// - If insufficient height delta: `reschedule_check`.
/// - Queries `registered_relay_socket_addrs()`, maps to `Peer::from_addr`.
/// - On error: warns `"failed to get ledger entries"`, reschedules.
/// - On success: sends `PeerSelectionMsg::LedgerCheckCandidates(...)` to parent
///   (via the captured `stage` ref), updates `last_height`, reschedules.
/// - `reschedule_check` always does `eff.schedule_after((), cadence)`.
///
/// The child is created exactly once on `Initialize` and communicates back only
/// via the `LedgerCheckCandidates` message.
///
/// ## Regulation, Schedules, and Effects
///
/// `regulate_peers` (called from `CheckCooldowns`, outbound non-retry disconnect,
/// `ConnectFailed`, `LedgerCheckCandidates`, and outbound removal inside `ban_peer`)
/// early-returns if `outbound_peers.len() >= target_upstream_peers`. Otherwise it
/// obtains a seed via `eff.external(GenerateRandomSeed)`, builds an `StdRng`, and
/// does three passes (statics, then snapshot_candidates, then ledger_candidates):
/// - Filters candidates to those absent from `outbound_peers` and `cooldown_until`.
/// - `choose_multiple` up to the deficit.
/// - For each: logs `"peer_selection.add_peer"`, sends `ManagerMessage::AddPeer`,
///   inserts as `Connecting`.
///
/// Schedules (via `Effects`):
/// - At most one `CheckCooldowns` armed via `schedule_at` (from `cool_down` /
///   `arm_next_cooldown`); remaining cool-downs live only in the min-heap.
/// - Child-internal `()` triggers (60s cadence, conditional on height delta).
///
/// Other effects used: `eff.send` (to manager and child), `eff.clock`, `eff.schedule_at`,
/// `eff.schedule_after`, `eff.cancel_schedule`, `eff.stage`/`eff.wire_up`, `eff.me()`,
/// `eff.external`, and `Ledger` (via effects facade).
///
/// ## Logging, Errors, and Invariants
///
/// - Structured logs at `info!`/`warn!`/`debug!` for key transitions (e.g.,
///   `peer_selection.connect_initial`, `add_peer` with `was_banned`, inbound
///   rejection, removals with `is_static`, outbound replacement warnings).
/// - Ledger child errors are logged at `warn!` but do not crash the parent
///   (just reschedule with no candidate update).
/// - Stale messages are tolerated (e.g., `CheckCooldowns` with nothing due still
///   runs `regulate_peers`; duplicate `AddPeer` is a no-op after logging).
/// - Cool-down invariants: live bans are only in `cooldown_until` (created in
///   `cool_down`, removed in `CheckCooldowns`/`AddPeer`); the heap may lag with
///   stale entries; at most one `cooldown_timer` is outstanding. Inbound/outbound
///   maps are updated only on exact id matches in disconnect paths.
///   `outbound_peers` length is the primary signal for regulation. `static_peers`
///   and `snapshot_candidates` are never mutated after `new`.
/// - `Connection` and `PeerState` are simple value types for tracking duplex
///   capability and lifecycle.
///
/// The stage is exercised via `test_setup.rs` (which overrides ledger effects and
/// `GenerateRandomSeed` for determinism, uses virtual child stages, and provides
/// trace helpers) and `tests.rs` (covering Initialize, every `PeerSelectionMsg`
/// arm, double-adversarial timer replacement, regulate preference/skipping,
/// will_retry vs. normal disconnect, inbound caps, etc.).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerSelection {
    target_upstream_peers: usize,
    target_downstream_peers: usize,
    manager: StageRef<ManagerMessage>,
    static_peers: BTreeSet<Peer>,
    snapshot_candidates: BTreeSet<Peer>,
    ledger_candidates: BTreeSet<Peer>,
    peer_removal_cooldown: Duration,
    cooldowns: Cooldowns,
    cooldown_timer: Option<ScheduleId>,
    inbound_peers: BTreeMap<Peer, Connection>,
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
    Adversarial(Peer, TraceContext),
    /// Manually add a peer, mostly for testing.
    AddPeer(Peer),
    /// Wake-up to drain cool-downs whose end time is at or before now, then re-arm the next.
    CheckCooldowns,
    /// A peer has connected and the peer_selection stage can start tracking it.
    ///
    /// This may be a downstream peer or the successful result of a connection attempt.
    Connected(Peer, Connection, ConnectionDirection),
    /// A peer has disconnected and the peer_selection stage can stop tracking it.
    Disconnected(Peer, ConnectionId, ConnectionDirection, bool),
    /// A (re)connection attempt has failed, the Manager has removed this peer.
    ConnectFailed(Peer),
    /// Internal message from ledger check with new candidates.
    LedgerCheckCandidates(BTreeSet<Peer>),
}

impl PeerSelectionMsg {
    /// Shortcut for creating an adversarial message when no trace context is available
    pub fn adversarial(peer: Peer) -> PeerSelectionMsg {
        PeerSelectionMsg::Adversarial(peer, Default::default())
    }
}

impl PeerSelection {
    pub fn new(
        manager: StageRef<ManagerMessage>,
        static_peers: BTreeSet<Peer>,
        snapshot_candidates: BTreeSet<Peer>,
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
            snapshot_candidates,
            peer_removal_cooldown: Duration::from_secs(peer_removal_cooldown_secs),
            cooldowns: Cooldowns::default(),
            cooldown_timer: None,
            inbound_peers: BTreeMap::new(),
            outbound_peers: BTreeMap::new(),
        }
    }
}

impl PeerSelection {
    async fn ban_peer(&mut self, peer: Peer, eff: &Effects<PeerSelectionMsg>) {
        let is_static = self.static_peers.contains(&peer);

        let mut send_remove = false;
        if let Some(peer_state) = self.inbound_peers.remove(&peer) {
            tracing::warn!(%peer, ?peer_state, is_static, "removing peer (inbound)");
            send_remove = true;
        }

        if let Some(peer_state) = self.outbound_peers.remove(&peer) {
            tracing::warn!(%peer, ?peer_state, is_static, "removing peer (outbound)");
            self.regulate_peers(eff).await;
            send_remove = true;
        }

        if send_remove {
            eff.send(&self.manager, ManagerMessage::RemovePeer(peer.clone())).await;
        }

        self.cool_down(peer, eff, is_static).await;
    }

    async fn cool_down(&mut self, peer: Peer, eff: &Effects<PeerSelectionMsg>, is_static: bool) {
        let ban_period = if is_static { STATIC_PEER_BAN_PERIOD } else { self.peer_removal_cooldown };
        let now = eff.clock().await;
        let when = now + ban_period;

        let was_empty = self.cooldowns.add_and_is_first(peer.clone(), when);

        match self.cooldown_timer {
            None if was_empty => {
                let id = eff.schedule_at(PeerSelectionMsg::CheckCooldowns, when).await;
                self.cooldown_timer = Some(id);
            }
            None => {
                self.arm_next_cooldown(eff).await;
            }
            Some(id) if id.time() > when => {
                eff.cancel_schedule(id).await;
                let id = eff.schedule_at(PeerSelectionMsg::CheckCooldowns, when).await;
                self.cooldown_timer = Some(id);
            }
            Some(_) => {}
        }
    }

    async fn arm_next_cooldown(&mut self, eff: &Effects<PeerSelectionMsg>) {
        self.cooldowns.discard_stale();
        if let Some((when, _)) = self.cooldowns.peek() {
            let id = eff.schedule_at(PeerSelectionMsg::CheckCooldowns, when).await;
            self.cooldown_timer = Some(id);
        }
    }

    async fn regulate_peers(&mut self, eff: &Effects<PeerSelectionMsg>) {
        let target_upstream_peers = self.target_upstream_peers;
        let outbound = self.outbound_peers.len();

        if outbound >= target_upstream_peers {
            return;
        }

        // NOTE: randomness only enters this way and tests override the effect
        let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
        let mut rng = StdRng::from_seed(seed);

        // Prefer static peers, then snapshot candidates, then live ledger candidates.
        if outbound < target_upstream_peers {
            let candidates = self
                .static_peers
                .iter()
                .filter(|p| !self.outbound_peers.contains_key(p) && !self.cooldowns.is_cooling(p))
                .cloned()
                .choose_multiple(&mut rng, target_upstream_peers - outbound);
            for peer in candidates {
                tracing::info!(%peer, was_banned = false, "peer_selection.add_peer");
                eff.send(&self.manager, ManagerMessage::AddPeer(peer.clone())).await;
                self.outbound_peers.insert(peer, PeerState::Connecting);
            }
        }

        let outbound = self.outbound_peers.len();
        if outbound < target_upstream_peers {
            let candidates = self
                .snapshot_candidates
                .iter()
                .filter(|p| !self.outbound_peers.contains_key(p) && !self.cooldowns.is_cooling(p))
                .cloned()
                .choose_multiple(&mut rng, target_upstream_peers - outbound);
            for peer in candidates {
                tracing::info!(%peer, was_banned = false, "peer_selection.add_peer");
                eff.send(&self.manager, ManagerMessage::AddPeer(peer.clone())).await;
                self.outbound_peers.insert(peer, PeerState::Connecting);
            }
        }

        let outbound = self.outbound_peers.len();
        if outbound < target_upstream_peers {
            let candidates = self
                .ledger_candidates
                .iter()
                .filter(|p| !self.outbound_peers.contains_key(p) && !self.cooldowns.is_cooling(p))
                .cloned()
                .choose_multiple(&mut rng, target_upstream_peers - outbound);
            for peer in candidates {
                tracing::info!(%peer, was_banned = false, "peer_selection.add_peer");
                eff.send(&self.manager, ManagerMessage::AddPeer(peer.clone())).await;
                self.outbound_peers.insert(peer, PeerState::Connecting);
            }
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Cooldowns {
    cooldown_until: BTreeMap<Peer, Instant>,
    cooldown_heap: BinaryHeap<Reverse<(Instant, Peer)>>,
}

impl Cooldowns {
    /// Adds a peer to the cooldowns, returning true if this was the first entry.
    fn add_and_is_first(&mut self, peer: Peer, until: Instant) -> bool {
        let was_empty = self.cooldown_until.is_empty();
        self.cooldown_until.insert(peer.clone(), until);
        self.cooldown_heap.push(Reverse((until, peer)));
        was_empty
    }

    fn discard_stale(&mut self) {
        while let Some(Reverse((when, peer))) = self.cooldown_heap.peek() {
            if let Some(until) = self.cooldown_until.get(peer)
                && until == when
            {
                break; // stop discarding when we find the next valid entry
            }
            self.cooldown_heap.pop();
        }
    }

    fn peek(&self) -> Option<(Instant, Peer)> {
        self.cooldown_heap.peek().map(|Reverse((when, peer))| (*when, peer.clone()))
    }

    fn drain_due(&mut self, now: Instant) {
        while let Some(Reverse((when, _))) = self.cooldown_heap.peek() {
            if *when > now {
                break;
            }
            let Some(Reverse((_when, peer))) = self.cooldown_heap.pop() else {
                break;
            };
            if self.cooldown_until.get(&peer).is_some_and(|until| *until <= now) {
                self.cooldown_until.remove(&peer);
            }
        }
    }

    fn is_cooling(&self, peer: &Peer) -> bool {
        self.cooldown_until.contains_key(peer)
    }

    /// Removes a peer from the cooldowns, returning true if this empties the cooldowns.
    fn remove_and_was_last(&mut self, peer: &Peer) -> bool {
        let was_banned = self.cooldown_until.remove(peer).is_some();
        if self.cooldown_until.is_empty() {
            self.cooldown_heap.clear();
            was_banned
        } else {
            false
        }
    }
}

impl PartialEq for Cooldowns {
    fn eq(&self, other: &Self) -> bool {
        self.cooldown_until == other.cooldown_until && {
            let mut left: Vec<_> = self.cooldown_heap.iter().collect();
            let mut right: Vec<_> = other.cooldown_heap.iter().collect();
            left.sort();
            right.sort();
            left == right
        }
    }
}

pub async fn stage(mut state: PeerSelection, msg: PeerSelectionMsg, eff: Effects<PeerSelectionMsg>) -> PeerSelection {
    match msg {
        PeerSelectionMsg::Initialize => {
            tracing::info!(
                static_peers = state.static_peers.len(),
                snapshot_peers = state.snapshot_candidates.len(),
                "peer_selection.connect_initial"
            );
            for p in &state.static_peers {
                eff.send(&state.manager, ManagerMessage::AddPeer(p.clone())).await;
                state.outbound_peers.insert(p.clone(), PeerState::Connecting);
            }
            // Fill any remaining outbound slots from snapshot (then ledger when available).
            state.regulate_peers(&eff).await;
            // NOTE: no supervision, failure in ledger-check shall tear down the node.
            let ledger_check = eff
                .wire_up(
                    eff.stage("peer-selection/ledger-check", get_ledger_candidates).await,
                    LedgerCheck::new(eff.me()),
                )
                .await;
            eff.send(&ledger_check, ()).await;
        }
        PeerSelectionMsg::Adversarial(peer, trace_context) => {
            tracing::debug!(%peer, "peer_selection.adversarial");
            let span = debug_span!(parent_context: trace_context, consensus::peer::BAN, peer = peer.clone());
            state.ban_peer(peer, &eff).instrument(span).await;
        }
        PeerSelectionMsg::CheckCooldowns => {
            if let Some(id) = state.cooldown_timer.take() {
                eff.cancel_schedule(id).await;
            }
            let now = eff.clock().await;
            state.cooldowns.drain_due(now);
            state.regulate_peers(&eff).await;
            state.arm_next_cooldown(&eff).await;
        }
        PeerSelectionMsg::AddPeer(peer) => {
            let was_banned = state.cooldowns.is_cooling(&peer);
            if state.cooldowns.remove_and_was_last(&peer)
                && let Some(id) = state.cooldown_timer.take()
            {
                eff.cancel_schedule(id).await;
            }

            if !state.outbound_peers.contains_key(&peer) {
                tracing::info!(%peer, was_banned, "peer_selection.add_peer");
                eff.send(&state.manager, ManagerMessage::AddPeer(peer.clone())).await;
                state.outbound_peers.insert(peer, PeerState::Connecting);
            } else {
                tracing::info!(%peer, "not adding peer because already added");
            }
        }
        PeerSelectionMsg::Connected(peer, connection, ConnectionDirection::Inbound) => {
            if state.inbound_peers.len() >= state.target_downstream_peers {
                tracing::info!(%peer, "rejecting inbound connection: too many peers");
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, connection.id)).await;
                return state;
            }
            let span = debug_span!(
                amaru::protocols::peer_selection::peer::CONNECTED,
                peer = &peer,
                conn_id = connection.id.as_u64(),
                direction = ConnectionDirection::Inbound,
                full_duplex_capable = connection.full_duplex_capable,
                full_duplex = connection.full_duplex,
            )
            .entered();
            let old = state.inbound_peers.insert(peer.clone(), connection);
            if let Some(conn) = old {
                tracing::info!(%peer, ?conn, "inbound connection replaced by peer");
                drop(span);
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, conn.id)).await;
            }
        }
        PeerSelectionMsg::Connected(peer, connection, ConnectionDirection::Outbound) => {
            let span = debug_span!(
                amaru::protocols::peer_selection::peer::CONNECTED,
                peer = &peer,
                conn_id = connection.id.as_u64(),
                direction = ConnectionDirection::Outbound,
                full_duplex_capable = connection.full_duplex_capable,
                full_duplex = connection.full_duplex,
            )
            .entered();
            let old = state.outbound_peers.insert(peer.clone(), PeerState::Connected(connection));
            if let Some(PeerState::Connected(conn)) = old {
                tracing::warn!(%peer, ?conn, "connected outbound while still connected");
                drop(span);
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, conn.id)).await;
            }
        }
        PeerSelectionMsg::Disconnected(peer, conn_id, ConnectionDirection::Inbound, _) => {
            let _span = debug_span!(
                amaru::protocols::peer_selection::peer::DISCONNECTED,
                peer = &peer,
                conn_id = conn_id.as_u64(),
                direction = ConnectionDirection::Inbound,
            )
            .entered();
            if let Entry::Occupied(entry) = state.inbound_peers.entry(peer)
                && entry.get().id == conn_id
            {
                entry.remove();
            }
        }
        PeerSelectionMsg::Disconnected(_, _, ConnectionDirection::Outbound, will_retry) if will_retry => {}
        PeerSelectionMsg::Disconnected(peer, conn_id, ConnectionDirection::Outbound, _) => {
            if let Entry::Occupied(entry) = state.outbound_peers.entry(peer.clone())
                && let PeerState::Connected(conn) = entry.get()
                && conn.id == conn_id
            {
                let span = debug_span!(
                    amaru::protocols::peer_selection::peer::DISCONNECTED,
                    peer = peer,
                    conn_id = conn_id.as_u64(),
                    direction = ConnectionDirection::Outbound,
                )
                .entered();
                entry.remove();
                drop(span);
                state.regulate_peers(&eff).await;
            }
        }
        PeerSelectionMsg::ConnectFailed(peer) => {
            state.outbound_peers.remove(&peer);
            state.regulate_peers(&eff).await;
        }
        PeerSelectionMsg::LedgerCheckCandidates(candidates) => {
            state.ledger_candidates = candidates;
            state.regulate_peers(&eff).await;
        }
    }
    state
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

#[tracing::instrument(level = "trace", skip_all, fields(last_height = %state.last_height))]
async fn get_ledger_candidates(mut state: LedgerCheck, _msg: (), eff: Effects<()>) -> LedgerCheck {
    let ledger = Ledger::new(eff.clone());
    let current_height = ledger.volatile_tip().await.block_height();
    if current_height < state.last_height + state.min_height_change {
        return reschedule_check(state, eff).await;
    }
    let ledger_entries = ledger.registered_relay_socket_addrs().await;
    let ledger_entries = match ledger_entries {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, "failed to get ledger entries");
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

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
