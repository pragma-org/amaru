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
    net::SocketAddr,
    time::Duration,
};

use amaru_kernel::{BlockHeight, Peer};
use amaru_observability::{Instrument, TraceContext, debug, debug_span, info, warn};
use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::{
    manager::ManagerMessage,
    peer_sharing::{SharePeersReply, ShareResult},
};
use amaru_pure_stage::{Effects, Instant, ScheduleId, StageRef};

pub use crate::performance::{DEFAULT_PEER_MIX, PeerMix, PeerMixParseError};
use crate::{
    effects::{GenerateRandomSeed, Ledger, LedgerOps, ResolvePeerCandidate, ResolvePeerCandidateResult},
    performance::{PeerSource, Performance, SelectOutboundParams, SharedIngestResult},
};

const STATIC_PEER_BAN_PERIOD: Duration = Duration::from_secs(10);
/// Delay after outbound connect before the first peer-sharing request.
pub const SHARE_REQUEST_INITIAL_DELAY: Duration = Duration::from_millis(100);
/// Interval between subsequent peer-sharing requests on a live outbound connection.
pub const SHARE_REQUEST_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How many peers to request per share call (network-spec amount is `Word8`).
pub const SHARE_REQUEST_AMOUNT: u8 = 20;

/// Peer selection stage for the Amaru consensus node.
///
/// This stage is responsible for maintaining the desired number of outbound (upstream)
/// and inbound (downstream) peer connections. It acts as the decision point that tells
/// the `Manager` (via `ManagerMessage`) which peers to `AddPeer` or `RemovePeer`/`Disconnect`,
/// while reacting to connection lifecycle events and adversarial signals.
///
/// Outbound candidate sources and the admin peer-mix formula live in the **Performance** resource
/// (installed only via `Performance::with_peer_sources` at construction). This stage keeps
/// connection maps and cool-downs, and asks Performance to select dials / share replies
/// (see EDR-031). Hard excludes for dials: peers already in `outbound_peers` or under cool-down.
/// Inbound connections are accepted up to `target_downstream_peers` (excess are
/// immediately rejected with a `Disconnect`).
///
/// The stage creates (on `Initialize`, with no supervision) a child stage
/// `"peer-selection/ledger-check"` running `get_ledger_candidates` (backed by `LedgerCheck`
/// state) that periodically queries the ledger, writes candidates into the Performance
/// resource, then nudges this stage with a payload-free [`PeerSelectionMsg::Regulate`].
///
/// ## State
///
/// - `target_upstream_peers`, `target_downstream_peers`: configuration targets.
/// - `peer_mix`: admin source mix formula ([`PeerMix`]).
/// - `manager`: `StageRef<ManagerMessage>` for all outbound commands.
/// - `static_peers`, `snapshot_candidates`, `ledger_candidates`, `shared_peers`: candidate pools.
/// - `peer_removal_cooldown`: duration for non-static bans.
/// - `cooldown_until`: `BTreeMap<Peer, Instant>` of active bans (end time per peer).
/// - `cooldown_heap`: min-heap of pending `(Instant, Peer)` cool-down entries
///   (may contain stale entries after re-ban or early `AddPeer` lift).
/// - `cooldown_timer`: at most one outstanding `schedule_at(CheckCooldowns)` for the
///   earliest heap entry (`None` when no cool-downs are pending).
/// - `inbound_peers`: `BTreeMap<Peer, Connection>` (downstream tracking).
/// - `outbound_peers`: `BTreeMap<Peer, PeerState>` (`Connecting` or `Connected(Connection)`).
/// - `share_reply`: contramap of this stage accepting [`ShareResult`] (shared by all
///   peer-sharing initiators; scheduling lives on each initiator with its connection).
///
/// ## Message Handling
///
/// All behaviour is implemented in the single `match msg` inside `pub async fn stage`.
/// The stage is purely message-driven; there are no background loops outside scheduled
/// messages and the child stage.
///
/// - **Initialize**: Required at startup. Logs
///   `"peer_selection.connect_initial"`. For every resolved static [`Peer`]: sends
///   `ManagerMessage::AddPeer` and records it as `PeerState::Connecting`. Host/SRV
///   static and snapshot candidates are started with [`ResolvePeerCandidate`] via
///   [`Effects::detach`](amaru_pure_stage::Effects::detach). Then `regulate_peers`
///   fills remaining outbound slots. Unconditionally wires a new child stage
///   `"peer-selection/ledger-check"` (via `eff.stage` + `eff.wire_up` with
///   `LedgerCheck::new(eff.me())`, no supervision) and sends `()` to kick it off;
///   "failure in ledger-check shall tear down the node".
///
/// - **Resolved**: DNS result for a bootstrap candidate. Ingests addresses into
///   Performance (static or snapshot origin), logs `peer_selection.peer.resolved`,
///   dials new peers, then `regulate_peers`.
///
/// - **Adversarial**: Debug-logs `peer_selection.peer.adversarial`. Delegates to
///   `ban_peer`: removes the peer from `inbound_peers` (if present;
///   warns `"removing peer (inbound)"`) and/or `outbound_peers` (if present; warns
///   `"removing peer (outbound)"`).
///   If any removal occurred, sends `ManagerMessage::RemovePeer` to the manager.
///   Always calls `cool_down` (which computes `STATIC_PEER_BAN_PERIOD` (10s) for
///   static peers vs. configured cooldown, pushes onto the cool-down min-heap, and
///   arms `eff.schedule_at(CheckCooldowns)` only when the live cool-down set goes from
///   empty to one item, or when the new end time is earlier than the currently armed timer).
///   After the ban is recorded, outbound removal refills via `regulate_peers` (the banned
///   peer is excluded, so a static peer is not immediately re-added while still connected).
///
/// - **AddPeer**: Manual/test hook. If the peer
///   has an active cool-down, removes it from `cooldown_until` (`was_banned = true`)
///   and cancels the armed timer if no cool-downs remain. If the peer is not already
///   in `outbound_peers`: logs `peer_selection.peer.added` (with `was_banned`), sends
///   `ManagerMessage::AddPeer`, and inserts as `Connecting`. Otherwise logs
///   `peer_selection.peer.add_skipped` with `reason="already_added"`.
///
/// - **CheckCooldowns**: Clears any armed timer (cancel is a no-op if this message was
///   the delivery), drains all due min-heap entries (removes a peer from `cooldown_until`
///   when its stored deadline is `<= now` — so a re-ban with a later deadline is kept),
///   calls `regulate_peers`, then arms the next earliest heap entry via `schedule_at`
///   (if any). A past `Instant` is delivered on the priority path as soon as the runtime
///   can run this stage again.
///
/// - **Connected** `(peer, conn, direction, advertisable)`:
///   Records latest handshake advertisability on the Performance resource, then:
///   - `Inbound`: If `inbound_peers.len() >= target_downstream_peers`, logs
///     `peer_selection.peer.add_skipped` with `reason="too_many_inbound"`, sends `ManagerMessage::Disconnect`,
///     and returns early (no insert). Otherwise inserts (or replaces a prior
///     connection for the same peer, sending `Disconnect` for the old one).
///   - `Outbound`: Inserts/updates as `PeerState::Connected(conn)`. If replacing
///     a prior `Connected` state, warns and sends `Disconnect` for the old conn.
///     When `advertisable`, starts peer-sharing on that connection
///     (`ManagerMessage::RequestSharePeers` with [`SHARE_REQUEST_INITIAL_DELAY`] /
///     [`SHARE_REQUEST_INTERVAL`]); the initiator owns the request cadence.
///
/// - **Disconnected**:
///   - `Inbound`: Removes from `inbound_peers` only on exact `ConnectionId` match
///     (via `Entry::Occupied` guard).
///   - `Outbound` + `will_retry == true`: If present as `PeerState::Connected` with
///     matching id, replaces it with `Connecting` so a reconnect handshake does
///     not race a stale live entry; then clears availability if nothing remains.
///   - `Outbound` + `will_retry == false`: Removes only if present as exactly
///     `PeerState::Connected` with matching id; then `regulate_peers`.
///     (Share-request timers die with the connection's peer-sharing stage.)
///
/// - **ConnectFailed**: Records a connection failure on Performance, removes the peer from
///   `outbound_peers` (any `PeerState`), then calls `regulate_peers`.
///
/// - **SharePeersResult**: Inserts learned addresses into `shared_peers`, then
///   `regulate_peers` (no reschedule — initiator keeps the cadence).
///
/// - **Regulate**: Calls `regulate_peers` (e.g. after the ledger-check child updates Performance).
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
/// - On success: writes the set into Performance via `set_ledger_candidates` (not via the
///   parent mailbox), then sends payload-free `PeerSelectionMsg::Regulate` so the parent
///   can refill outbound slots, updates `last_height`, reschedules.
/// - `reschedule_check` always does `eff.schedule_after((), cadence)`.
///
/// The child is created exactly once on `Initialize`.
///
/// ## Regulation, Schedules, and Effects
///
/// `regulate_peers` (called from `CheckCooldowns`, outbound non-retry disconnect,
/// `ConnectFailed`, `Regulate`, and outbound removal inside `ban_peer` after the
/// ban is recorded)
/// early-returns if `outbound_peers.len() >= target_upstream_peers`. Otherwise it
/// obtains a seed via `eff.external(GenerateRandomSeed)` and asks Performance to
/// select dials (mix + quality-weighted sample within each
/// source (canonical origin; hard exclude outbound + cool-down).
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerSelection {
    target_upstream_peers: usize,
    target_downstream_peers: usize,
    manager: StageRef<ManagerMessage>,
    peer_removal_cooldown: Duration,
    cooldowns: Cooldowns,
    cooldown_timer: Option<ScheduleId>,
    inbound_peers: BTreeMap<Peer, Connection>,
    outbound_peers: BTreeMap<Peer, PeerState>,
    /// Contramap target for peer-sharing replies ([`ShareResult`] → [`PeerSelectionMsg::SharePeersResult`]).
    /// Ignored in [`PartialEq`] (lazily wired, test-unstable name).
    share_reply: StageRef<ShareResult>,
}

impl PartialEq for PeerSelection {
    fn eq(&self, other: &Self) -> bool {
        self.target_upstream_peers == other.target_upstream_peers
            && self.target_downstream_peers == other.target_downstream_peers
            && self.manager == other.manager
            && self.peer_removal_cooldown == other.peer_removal_cooldown
            && self.cooldowns == other.cooldowns
            && self.cooldown_timer == other.cooldown_timer
            && self.inbound_peers == other.inbound_peers
            && self.outbound_peers == other.outbound_peers
        // share_reply intentionally omitted
    }
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
    /// `advertisable` is the remote handshake peer-sharing willingness (latest wins in Performance).
    Connected(Peer, Connection, ConnectionDirection, bool),
    /// A peer has disconnected and the peer_selection stage can stop tracking it.
    Disconnected(Peer, ConnectionId, ConnectionDirection, bool),
    /// A (re)connection attempt has failed, the Manager has removed this peer.
    ConnectFailed(Peer),
    /// Ask the stage to refill outbound slots (no payload).
    ///
    /// Used by the ledger-check child after it has already written candidates into Performance.
    Regulate,
    /// Reply from the peer-sharing initiator (one result per request cycle).
    SharePeersResult { peer: Peer, peers: Vec<SocketAddr> },
    /// Server-side peer-sharing: select addresses to advertise to `peer` and reply on `reply_to`.
    ShareRequest { peer: Peer, amount: u8, reply_to: StageRef<SharePeersReply> },
    /// DNS result for a bootstrap [`amaru_kernel::PeerCandidate`].
    Resolved(ResolvePeerCandidateResult),
}

impl PeerSelectionMsg {
    /// Shortcut for creating an adversarial message when no trace context is available
    pub fn adversarial(peer: Peer) -> PeerSelectionMsg {
        PeerSelectionMsg::Adversarial(peer, Default::default())
    }
}

impl PeerSelection {
    /// Construct connection-selection state only.
    ///
    /// Outbound candidate sources and the peer-mix formula are installed exclusively when
    /// constructing the [`Performance`] resource
    /// (`with_peer_sources`), never via live reconfiguration.
    pub fn new(
        manager: StageRef<ManagerMessage>,
        target_upstream_peers: usize,
        target_downstream_peers: usize,
        peer_removal_cooldown_secs: u64,
    ) -> Self {
        Self {
            target_upstream_peers,
            target_downstream_peers,
            manager,
            peer_removal_cooldown: Duration::from_secs(peer_removal_cooldown_secs),
            cooldowns: Cooldowns::default(),
            cooldown_timer: None,
            inbound_peers: BTreeMap::new(),
            outbound_peers: BTreeMap::new(),
            share_reply: StageRef::blackhole(),
        }
    }
}

impl PeerSelection {
    /// Whether this peer still has a usable connection (inbound or established outbound).
    fn peer_still_connected(&self, peer: &Peer) -> bool {
        self.inbound_peers.contains_key(peer) || matches!(self.outbound_peers.get(peer), Some(PeerState::Connected(_)))
    }

    async fn ban_peer(&mut self, peer: Peer, eff: &Effects<PeerSelectionMsg>) {
        let is_static = eff.external(Performance::is_static_peer(peer)).await;

        let mut send_remove = false;
        let mut refill_outbound = false;
        if let Some(peer_state) = self.inbound_peers.remove(&peer) {
            warn!(
                protocols::peer_selection::peer::REMOVED,
                peer,
                direction = "inbound",
                peer_state = format!("{peer_state:?}"),
                is_static
            );
            send_remove = true;
        }

        if let Some(peer_state) = self.outbound_peers.remove(&peer) {
            warn!(
                protocols::peer_selection::peer::REMOVED,
                peer,
                direction = "outbound",
                peer_state = format!("{peer_state:?}"),
                is_static
            );
            send_remove = true;
            refill_outbound = true;
        }

        if send_remove {
            eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
        }

        let now = eff.clock().await;
        eff.external(Performance::peer_adversarial(peer, now)).await;
        self.cool_down(peer, eff, is_static, now).await;
        if refill_outbound {
            self.regulate_peers(eff).await;
        }
    }

    /// Drop availability claims when a peer has no remaining live connections (scores kept).
    async fn clear_availability_if_gone(&self, peer: &Peer, eff: &Effects<PeerSelectionMsg>) {
        if !self.peer_still_connected(peer) {
            eff.external(Performance::clear_peer_availability(*peer)).await;
        }
    }

    async fn cool_down(&mut self, peer: Peer, eff: &Effects<PeerSelectionMsg>, is_static: bool, now: Instant) {
        let ban_period = if is_static { STATIC_PEER_BAN_PERIOD } else { self.peer_removal_cooldown };
        let when = now + ban_period;

        let was_empty = self.cooldowns.add_and_is_first(peer, when);

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
        let open = target_upstream_peers - outbound;

        let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
        let now = eff.clock().await;
        let mut excluded = self.outbound_peers.keys().cloned().collect::<BTreeSet<_>>();
        for p in self.cooldowns.cooling_peers() {
            excluded.insert(p);
        }
        let picked =
            eff.external(Performance::select_outbound(SelectOutboundParams { open, excluded, seed, now })).await;
        for peer in picked {
            if self.outbound_peers.contains_key(&peer) {
                continue;
            }
            info!(protocols::peer_selection::peer::ADDED, peer, was_banned = false);
            eff.send(&self.manager, ManagerMessage::AddPeer(peer)).await;
            self.outbound_peers.insert(peer, PeerState::Connecting);
        }
    }

    /// Start peer-sharing on an outbound connection (initiator owns the request cadence).
    async fn start_peer_sharing(&mut self, peer: Peer, eff: &Effects<PeerSelectionMsg>) {
        if self.share_reply.is_blackhole() {
            self.share_reply = eff
                .me_ref()
                .contramap(|ShareResult { peer, peers }| PeerSelectionMsg::SharePeersResult { peer, peers });
        }
        eff.send(
            &self.manager,
            ManagerMessage::RequestSharePeers {
                peer,
                amount: SHARE_REQUEST_AMOUNT,
                initial_delay: SHARE_REQUEST_INITIAL_DELAY,
                interval: SHARE_REQUEST_INTERVAL,
                reply_to: self.share_reply.clone(),
            },
        )
        .await;
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
        self.cooldown_until.insert(peer, until);
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
        self.cooldown_heap.peek().map(|Reverse((when, peer))| (*when, *peer))
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

    fn cooling_peers(&self) -> impl Iterator<Item = Peer> + '_ {
        self.cooldown_until.keys().cloned()
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
            let counts = eff.external(Performance::source_counts()).await;
            info!(
                protocols::peer_selection::CONNECT_INITIAL,
                static_peers = counts.static_peers,
                snapshot_peers = counts.snapshot_candidates
            );
            let static_peers = eff.external(Performance::static_peers()).await;
            for p in static_peers {
                eff.send(&state.manager, ManagerMessage::AddPeer(p)).await;
                state.outbound_peers.insert(p, PeerState::Connecting);
            }
            let unresolved_static = eff.external(Performance::unresolved_static()).await;
            for candidate in unresolved_static {
                eff.detach(ResolvePeerCandidate::new(candidate, PeerSource::Static), PeerSelectionMsg::Resolved).await;
            }
            let unresolved_snapshot = eff.external(Performance::unresolved_snapshot()).await;
            for candidate in unresolved_snapshot {
                eff.detach(ResolvePeerCandidate::new(candidate, PeerSource::Snapshot), PeerSelectionMsg::Resolved)
                    .await;
            }
            // Fill remaining outbound slots via mix + quality selection in Performance.
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
            debug!(protocols::peer_selection::peer::ADVERSARIAL, peer);
            let span = debug_span!(parent_context: trace_context, consensus::peer::BAN, peer);
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
                info!(protocols::peer_selection::peer::ADDED, peer, was_banned);
                eff.send(&state.manager, ManagerMessage::AddPeer(peer)).await;
                state.outbound_peers.insert(peer, PeerState::Connecting);
            } else {
                info!(protocols::peer_selection::peer::ADD_SKIPPED, peer, reason = "already_added");
            }
        }
        PeerSelectionMsg::Connected(peer, connection, ConnectionDirection::Inbound, advertisable) => {
            let now = eff.clock().await;
            eff.external(Performance::record_advertisability(peer, advertisable, now)).await;
            if state.inbound_peers.len() >= state.target_downstream_peers {
                info!(protocols::peer_selection::peer::ADD_SKIPPED, peer, reason = "too_many_inbound");
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, connection.id)).await;
                return state;
            }
            let span = debug_span!(
                amaru::protocols::peer_selection::peer::CONNECTED,
                peer,
                conn_id = connection.id.as_u64(),
                direction = ConnectionDirection::Inbound,
                full_duplex_capable = connection.full_duplex_capable,
                full_duplex = connection.full_duplex,
            )
            .entered();
            let old = state.inbound_peers.insert(peer, connection);
            if let Some(conn) = old {
                info!(
                    protocols::peer_selection::peer::RECONNECTED,
                    peer,
                    direction = "inbound",
                    conn_id = conn.id.as_u64()
                );
                drop(span);
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, conn.id)).await;
            }
        }
        PeerSelectionMsg::Connected(peer, connection, ConnectionDirection::Outbound, advertisable) => {
            let now = eff.clock().await;
            eff.external(Performance::record_advertisability(peer, advertisable, now)).await;
            let span = debug_span!(
                amaru::protocols::peer_selection::peer::CONNECTED,
                peer,
                conn_id = connection.id.as_u64(),
                direction = ConnectionDirection::Outbound,
                full_duplex_capable = connection.full_duplex_capable,
                full_duplex = connection.full_duplex,
            )
            .entered();
            let old = state.outbound_peers.insert(peer, PeerState::Connected(connection));
            let disconnect_old = if let Some(PeerState::Connected(conn)) = old {
                warn!(
                    protocols::peer_selection::peer::RECONNECTED,
                    peer,
                    direction = "outbound",
                    conn_id = conn.id.as_u64()
                );
                Some(conn.id)
            } else {
                None
            };
            drop(span);
            if let Some(old_id) = disconnect_old {
                eff.send(&state.manager, ManagerMessage::Disconnect(peer, old_id)).await;
            }
            // Only ask peers that advertised peer-sharing willingness (they run the server).
            // Cadence lives on the peer-sharing initiator until the connection ends.
            if advertisable {
                state.start_peer_sharing(peer, &eff).await;
            }
        }
        PeerSelectionMsg::Disconnected(peer, conn_id, ConnectionDirection::Inbound, _) => {
            {
                let _span = debug_span!(
                    amaru::protocols::peer_selection::peer::DISCONNECTED,
                    peer,
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
            state.clear_availability_if_gone(&peer, &eff).await;
        }
        PeerSelectionMsg::Disconnected(peer, conn_id, ConnectionDirection::Outbound, true) => {
            if let Entry::Occupied(mut entry) = state.outbound_peers.entry(peer)
                && let PeerState::Connected(conn) = entry.get()
                && conn.id == conn_id
            {
                let _span = debug_span!(
                    amaru::protocols::peer_selection::peer::DISCONNECTED,
                    peer,
                    conn_id = conn_id.as_u64(),
                    direction = ConnectionDirection::Outbound,
                )
                .entered();
                entry.insert(PeerState::Connecting);
            }
            state.clear_availability_if_gone(&peer, &eff).await;
        }
        PeerSelectionMsg::Disconnected(peer, conn_id, ConnectionDirection::Outbound, _) => {
            if let Entry::Occupied(entry) = state.outbound_peers.entry(peer)
                && let PeerState::Connected(conn) = entry.get()
                && conn.id == conn_id
            {
                let span = debug_span!(
                    amaru::protocols::peer_selection::peer::DISCONNECTED,
                    peer,
                    conn_id = conn_id.as_u64(),
                    direction = ConnectionDirection::Outbound,
                )
                .entered();
                entry.remove();
                drop(span);
                state.clear_availability_if_gone(&peer, &eff).await;
                state.regulate_peers(&eff).await;
            }
        }
        PeerSelectionMsg::ConnectFailed(peer) => {
            let now = eff.clock().await;
            eff.external(Performance::record_connection_failure(peer, now)).await;
            state.outbound_peers.remove(&peer);
            state.clear_availability_if_gone(&peer, &eff).await;
            state.regulate_peers(&eff).await;
        }
        PeerSelectionMsg::SharePeersResult { peer, peers } => {
            // FIXME emit array once observability supports it
            let peers_list = peers.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let SharedIngestResult { added, total } = eff.external(Performance::ingest_shared_peers(peer, peers)).await;
            info!(protocols::peer_selection::sharing::RECEIVED, peer, peers = peers_list, added, total,);
            if added > 0 {
                state.regulate_peers(&eff).await;
            }
        }
        PeerSelectionMsg::Regulate => {
            state.regulate_peers(&eff).await;
        }
        PeerSelectionMsg::ShareRequest { peer, amount, reply_to } => {
            let now = eff.clock().await;
            let selected = eff.external(Performance::select_share_peers(peer, amount, now)).await;
            let peers_list = selected.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let count = selected.len();
            info!(protocols::peer_selection::sharing::SENT, peer, peers = peers_list, requested = amount, count,);
            eff.send(&reply_to, SharePeersReply { peers: selected }).await;
        }
        PeerSelectionMsg::Resolved(ResolvePeerCandidateResult { candidate, origin, peers }) => {
            let peers_list = peers.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            info!(
                protocols::peer_selection::peer::RESOLVED,
                candidate = candidate.to_string(),
                origin = origin.as_str(),
                peers = peers_list,
                count = peers.len(),
            );
            eff.external(Performance::ingest_resolved(origin, candidate, peers.clone())).await;
            for peer in peers {
                if !state.outbound_peers.contains_key(&peer) {
                    info!(protocols::peer_selection::peer::ADDED, peer, was_banned = false);
                    eff.send(&state.manager, ManagerMessage::AddPeer(peer)).await;
                    state.outbound_peers.insert(peer, PeerState::Connecting);
                }
            }
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

async fn get_ledger_candidates(state: LedgerCheck, msg: (), eff: Effects<()>) -> LedgerCheck {
    let span =
        debug_span!(protocols::peer_selection::ledger::CHECK_CANDIDATES, last_height = state.last_height.as_u64());
    get_ledger_candidates_inner(state, msg, eff).instrument(span).await
}

async fn get_ledger_candidates_inner(mut state: LedgerCheck, _msg: (), eff: Effects<()>) -> LedgerCheck {
    let ledger = Ledger::new(eff.clone());
    let current_height = ledger.volatile_tip().await.block_height();
    if current_height < state.last_height + state.min_height_change {
        return reschedule_check(state, eff).await;
    }
    let ledger_entries = ledger.registered_relay_socket_addrs().await;
    let ledger_entries = match ledger_entries {
        Ok(entries) => entries,
        Err(error) => {
            warn!(protocols::peer_selection::ledger::CANDIDATES_FAILED, error = error.to_string());
            return reschedule_check(state, eff).await;
        }
    };
    let ledger_entries = ledger_entries
        .into_iter()
        .filter_map(|entry| match Peer::try_from(entry) {
            Ok(peer) => Some(peer),
            Err(reason) => {
                warn!(
                    protocols::peer_selection::peer::ADDRESS_REJECTED,
                    address = entry.to_string(),
                    reason = reason.to_string()
                );
                None
            }
        })
        .collect();
    // Keep the large candidate set out of the parent stage mailbox / TraceBuffer path.
    eff.external(Performance::set_ledger_candidates(ledger_entries)).await;
    eff.send(&state.stage, PeerSelectionMsg::Regulate).await;
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
