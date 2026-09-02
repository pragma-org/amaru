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

use amaru_kernel::{BlockHeight, Peer, PeerCandidate};
use amaru_observability::{Instrument, TraceContext, debug, debug_span, info, warn};
use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::{
    connection::LocalUse,
    manager::ManagerMessage,
    peer_sharing::{SharePeersReply, ShareResult},
};
use amaru_pure_stage::{Effects, Instant, ScheduleId, StageRef};

pub use crate::performance::{DEFAULT_PEER_MIX, PeerMix, PeerMixParseError};
use crate::{
    effects::{GenerateRandomSeed, Ledger, LedgerOps, ResolvePeerCandidate, ResolvePeerCandidateResult},
    performance::{Performance, SelectOutboundParams, SharedIngestResult},
};

const STATIC_PEER_BAN_PERIOD: Duration = Duration::from_secs(10);
/// Backoff after a failed Host/SRV lookup before that candidate may be picked again.
const RESOLUTION_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Delay after outbound connect before the first peer-sharing request.
pub const SHARE_REQUEST_INITIAL_DELAY: Duration = Duration::from_secs(300);
/// Interval between subsequent peer-sharing requests on a live outbound connection.
pub const SHARE_REQUEST_INTERVAL: Duration = Duration::from_secs(900);
/// How many peers to request per share call (network-spec amount is `Word8`).
pub const SHARE_REQUEST_AMOUNT: u8 = 20;
/// Caught-up churn interval before fuzz (Haskell default).
pub const CHURN_INTERVAL_BASE: Duration = Duration::from_secs(3300);
/// Extra delay drawn uniformly from `0..=CHURN_INTERVAL_FUZZ`.
pub const CHURN_INTERVAL_FUZZ: Duration = Duration::from_secs(600);
/// Fraction of Using peers to demote each cycle (at least one).
pub const CHURN_FRACTION_PERCENT: usize = 20;
/// After clean churn, the bearer stays; do not re-promote for this long.
pub const CHURN_REPROMOTE_DELAY: Duration = Duration::from_secs(10);
/// Retry Using after no intersection (not hostility).
pub const UNINTERESTING_RETRY: Duration = Duration::from_secs(120);
/// Retry Using after a rollback past the intersection.
pub const UNINTERESTING_RETRY_AFTER_ROLLBACK: Duration = Duration::from_secs(180);

fn churn_interval(seed: [u8; 32]) -> Duration {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&seed[0..8]);
    let fuzz_secs = u64::from_le_bytes(bytes) % (CHURN_INTERVAL_FUZZ.as_secs() + 1);
    CHURN_INTERVAL_BASE + Duration::from_secs(fuzz_secs)
}

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
/// - `pending_resolve`: Host/SRV candidates currently being resolved (count toward occupancy).
/// - `bound`: Host/SRV candidates whose current outbound dial came from them (excluded from mix
///   until that peer leaves outbound, so the name can be resolved again later).
/// - `resolve_backoff`: failed Host/SRV lookups excluded until the given instant.
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
///   `"peer_selection.connect_initial"`. Then `regulate_peers` fills outbound slots
///   from the mix (static included). Host/SRV names are resolved just before
///   dialling and remain in their source pool so a later pick re-resolves.
///   Unconditionally wires a new child stage `"peer-selection/ledger-check"`
///   (via `eff.stage` + `eff.wire_up` with `LedgerCheck::new(eff.me())`, no
///   supervision) and sends `()` to kick it off; "failure in ledger-check shall
///   tear down the node".
///
/// - **Resolved**: DNS result for a selected bootstrap candidate (at most one
///   [`Peer`]). On success, notes the dial origin for malus, dials that address,
///   then `regulate_peers`. The candidate stays in its pool. On failure, the
///   candidate is held in `resolve_backoff` for 30s, other slots are refilled
///   immediately, and a delayed [`PeerSelectionMsg::Regulate`] is armed if the
///   outbound target is still short.
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
///     Sends `SetLocalUse(Diffusion)` so fetch/share follow actual local use.
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
/// - Queries `registered_relay_candidates()`, writes [`PeerCandidate`]s (sockets, hostnames, SRV).
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
/// `ConnectFailed`, `Regulate`, churn/uninteresting demotion, and outbound removal
/// inside `ban_peer` after the ban is recorded)
/// early-returns if Using occupancy (`Diffusion` outbound + in-flight dials) is at
/// `target_upstream_peers`. Eligible Maintenance outbound is promoted first. Otherwise it
/// obtains a seed via `eff.external(GenerateRandomSeed)` and asks Performance to
/// select [`PeerCandidate`]s (mix + quality-weighted sample within each source;
/// hard exclude outbound + cool-down + in-flight resolve). Socket candidates are
/// dialled immediately; Host/SRV candidates are resolved via
/// [`ResolvePeerCandidate`] and dialled when [`PeerSelectionMsg::Resolved`] arrives.
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
    /// Host/SRV candidates with an in-flight [`ResolvePeerCandidate`] (not yet dialled).
    pending_resolve: BTreeSet<PeerCandidate>,
    /// Host/SRV candidates currently bound to an outbound [`Peer`] (re-resolved after unbind).
    bound: BTreeMap<PeerCandidate, Peer>,
    /// Failed Host/SRV lookups that must not be re-selected until the stored instant.
    resolve_backoff: BTreeMap<PeerCandidate, Instant>,
    /// Contramap target for peer-sharing replies ([`ShareResult`] → [`PeerSelectionMsg::SharePeersResult`]).
    /// Ignored in [`PartialEq`] (lazily wired, test-unstable name).
    share_reply: StageRef<ShareResult>,
    /// Next regular churn wake. Ignored in [`PartialEq`] (schedule id is test-unstable).
    churn_timer: Option<ScheduleId>,
    /// Peers demoted from Using that must not be re-promoted until this instant.
    demoted_until: BTreeMap<Peer, Instant>,
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
            && self.pending_resolve == other.pending_resolve
            && self.bound == other.bound
            && self.resolve_backoff == other.resolve_backoff
            && self.demoted_until == other.demoted_until
        // share_reply and churn_timer intentionally omitted
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
    local_use: LocalUse,
}

impl Connection {
    pub fn new(id: ConnectionId, full_duplex_capable: bool, full_duplex: bool) -> Self {
        Self { id, full_duplex_capable, full_duplex, local_use: LocalUse::None }
    }

    pub fn with_local_use(mut self, local_use: LocalUse) -> Self {
        self.local_use = local_use;
        self
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
    /// Periodic Using-set churn: demote the worst non-static fraction, then refill.
    Churn,
    /// ChainSync found no usable intersection (or rolled back past it). Stop diffusion, keep the bearer.
    Uninteresting { peer: Peer, conn_id: ConnectionId, after_rollback: bool },
    /// Reconsider a previously demoted outbound bearer as Using.
    Promote { peer: Peer, conn_id: ConnectionId },
    /// Reply from the peer-sharing initiator (one result per request cycle).
    SharePeersResult { peer: Peer, peers: Vec<SocketAddr> },
    /// Server-side peer-sharing: select addresses to advertise to `peer` and reply on `reply_to`.
    ShareRequest { peer: Peer, amount: u8, reply_to: StageRef<SharePeersReply> },
    /// DNS result for a selected bootstrap [`amaru_kernel::PeerCandidate`] (at most one [`Peer`]).
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
            pending_resolve: BTreeSet::new(),
            bound: BTreeMap::new(),
            resolve_backoff: BTreeMap::new(),
            share_reply: StageRef::blackhole(),
            churn_timer: None,
            demoted_until: BTreeMap::new(),
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
            self.unbind_peer(&peer);
            self.demoted_until.remove(&peer);
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

    fn unbind_peer(&mut self, peer: &Peer) {
        self.bound.retain(|_, bound| bound != peer);
    }

    async fn start_dial(
        &mut self,
        candidate: PeerCandidate,
        origin: crate::performance::PeerSource,
        peer: Peer,
        eff: &Effects<PeerSelectionMsg>,
    ) {
        eff.external(Performance::note_dial(origin, candidate.clone(), peer)).await;
        if candidate.needs_resolution() {
            self.bound.insert(candidate, peer);
        }
        info!(protocols::peer_selection::peer::ADDED, peer, was_banned = false);
        eff.send(&self.manager, ManagerMessage::AddPeer(peer)).await;
        self.outbound_peers.insert(peer, PeerState::Connecting);
    }

    fn using_occupancy(&self) -> usize {
        self.pending_resolve.len()
            + self
                .outbound_peers
                .values()
                .filter(|state| match state {
                    PeerState::Connecting => true,
                    PeerState::Connected(conn) => conn.local_use == LocalUse::Diffusion,
                })
                .count()
    }

    fn using_peers(&self) -> Vec<Peer> {
        self.outbound_peers
            .iter()
            .filter_map(|(peer, state)| match state {
                PeerState::Connected(conn) if conn.local_use == LocalUse::Diffusion => Some(*peer),
                PeerState::Connecting | PeerState::Connected(_) => None,
            })
            .collect()
    }

    async fn arm_churn(&mut self, eff: &Effects<PeerSelectionMsg>) {
        let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
        let now = eff.clock().await;
        let id = eff.schedule_at(PeerSelectionMsg::Churn, now + churn_interval(seed)).await;
        self.churn_timer = Some(id);
    }

    async fn demote_to_maintenance(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        reason: &'static str,
        until: Instant,
        eff: &Effects<PeerSelectionMsg>,
    ) -> bool {
        let Some(PeerState::Connected(conn)) = self.outbound_peers.get_mut(&peer) else {
            return false;
        };
        if conn.id != conn_id || conn.local_use != LocalUse::Diffusion {
            return false;
        }
        conn.local_use = LocalUse::Maintenance;
        self.demoted_until.insert(peer, until);
        info!(protocols::peer_selection::peer::DEMOTED, peer, conn_id = conn_id.as_u64(), reason);
        eff.send(&self.manager, ManagerMessage::SetLocalUse { peer, conn_id, local_use: LocalUse::Maintenance }).await;
        let _promote = eff.schedule_at(PeerSelectionMsg::Promote { peer, conn_id }, until).await;
        true
    }

    async fn try_promote(&mut self, peer: Peer, conn_id: ConnectionId, now: Instant, eff: &Effects<PeerSelectionMsg>) {
        if self.demoted_until.get(&peer).is_some_and(|until| *until > now) {
            return;
        }
        self.demoted_until.remove(&peer);
        if self.using_occupancy() >= self.target_upstream_peers {
            return;
        }
        let Some(PeerState::Connected(conn)) = self.outbound_peers.get_mut(&peer) else {
            return;
        };
        if conn.id != conn_id || conn.local_use != LocalUse::Maintenance {
            return;
        }
        conn.local_use = LocalUse::Diffusion;
        eff.send(&self.manager, ManagerMessage::SetLocalUse { peer, conn_id, local_use: LocalUse::Diffusion }).await;
    }

    async fn churn(&mut self, eff: &Effects<PeerSelectionMsg>) {
        let using = self.using_peers();
        if using.is_empty() {
            return;
        }
        let want = (using.len() * CHURN_FRACTION_PERCENT / 100).max(1).min(using.len());
        let now = eff.clock().await;
        let ranked = eff.external(Performance::rank_peers_for_churn(using, now)).await;
        let mut demoted = 0;
        for (peer, _) in ranked {
            if demoted >= want {
                break;
            }
            if eff.external(Performance::is_static_peer(peer)).await {
                continue;
            }
            let Some(PeerState::Connected(conn)) = self.outbound_peers.get(&peer) else {
                continue;
            };
            if conn.local_use != LocalUse::Diffusion {
                continue;
            }
            let conn_id = conn.id;
            if self.demote_to_maintenance(peer, conn_id, "churn", now + CHURN_REPROMOTE_DELAY, eff).await {
                demoted += 1;
            }
        }
        if demoted > 0 {
            self.regulate_peers(eff).await;
        }
    }

    async fn promote_eligible_maintenance(&mut self, now: Instant, eff: &Effects<PeerSelectionMsg>) {
        let candidates: Vec<(Peer, ConnectionId)> = self
            .outbound_peers
            .iter()
            .filter_map(|(peer, state)| match state {
                PeerState::Connected(conn)
                    if conn.local_use == LocalUse::Maintenance
                        && !self.demoted_until.get(peer).is_some_and(|until| *until > now) =>
                {
                    Some((*peer, conn.id))
                }
                PeerState::Connecting | PeerState::Connected(_) => None,
            })
            .collect();
        for (peer, conn_id) in candidates {
            if self.using_occupancy() >= self.target_upstream_peers {
                break;
            }
            self.try_promote(peer, conn_id, now, eff).await;
        }
    }

    async fn regulate_peers(&mut self, eff: &Effects<PeerSelectionMsg>) {
        let now = eff.clock().await;
        self.promote_eligible_maintenance(now, eff).await;
        let target_upstream_peers = self.target_upstream_peers;
        let outbound = self.using_occupancy();
        if outbound >= target_upstream_peers {
            return;
        }
        let open = target_upstream_peers - outbound;

        let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
        let now = eff.clock().await;
        let mut excluded: BTreeSet<PeerCandidate> =
            self.outbound_peers.keys().copied().map(PeerCandidate::from).collect();
        for p in self.cooldowns.cooling_peers() {
            excluded.insert(PeerCandidate::from(p));
        }
        excluded.extend(self.pending_resolve.iter().cloned());
        excluded.extend(self.bound.keys().cloned());
        self.resolve_backoff.retain(|_, until| *until > now);
        excluded.extend(self.resolve_backoff.keys().cloned());
        let picked =
            eff.external(Performance::select_outbound(SelectOutboundParams { open, excluded, seed, now })).await;
        for pick in picked {
            match pick.candidate.as_peer() {
                Some(peer) => {
                    if self.outbound_peers.contains_key(&peer) {
                        continue;
                    }
                    self.start_dial(pick.candidate, pick.origin, peer, eff).await;
                }
                None => {
                    if !self.pending_resolve.insert(pick.candidate.clone()) {
                        continue;
                    }
                    eff.detach(ResolvePeerCandidate::new(pick.candidate, pick.origin), PeerSelectionMsg::Resolved)
                        .await;
                }
            }
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
            // Mix includes static sockets and names; Host/SRV are resolved just before dialling
            // and stay in their pool so a later pick re-resolves.
            state.regulate_peers(&eff).await;
            state.arm_churn(&eff).await;
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
            let mut connection = connection;
            connection.local_use = LocalUse::Diffusion;
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
            eff.send(
                &state.manager,
                ManagerMessage::SetLocalUse { peer, conn_id: connection.id, local_use: LocalUse::Diffusion },
            )
            .await;
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
                state.unbind_peer(&peer);
                state.demoted_until.remove(&peer);
                state.clear_availability_if_gone(&peer, &eff).await;
                state.regulate_peers(&eff).await;
            }
        }
        PeerSelectionMsg::ConnectFailed(peer) => {
            let now = eff.clock().await;
            eff.external(Performance::record_connection_failure(peer, now)).await;
            state.outbound_peers.remove(&peer);
            state.unbind_peer(&peer);
            state.demoted_until.remove(&peer);
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
        PeerSelectionMsg::Resolved(ResolvePeerCandidateResult { candidate, origin, peer }) => {
            state.pending_resolve.remove(&candidate);
            let Some(peer) = peer else {
                let now = eff.clock().await;
                state.resolve_backoff.insert(candidate, now + RESOLUTION_RETRY_DELAY);
                state.regulate_peers(&eff).await;
                if state.using_occupancy() < state.target_upstream_peers {
                    eff.schedule_at(PeerSelectionMsg::Regulate, now + RESOLUTION_RETRY_DELAY).await;
                }
                return state;
            };
            info!(
                protocols::peer_selection::peer::RESOLVED,
                candidate = candidate.to_string(),
                origin = origin.as_str(),
                peer,
            );
            if state.cooldowns.is_cooling(&peer) {
                return state;
            }
            if state.outbound_peers.contains_key(&peer) {
                state.regulate_peers(&eff).await;
                return state;
            }
            state.start_dial(candidate, origin, peer, &eff).await;
            state.regulate_peers(&eff).await;
        }
        PeerSelectionMsg::Churn => {
            if let Some(id) = state.churn_timer.take() {
                eff.cancel_schedule(id).await;
            }
            state.churn(&eff).await;
            state.arm_churn(&eff).await;
        }
        PeerSelectionMsg::Uninteresting { peer, conn_id, after_rollback } => {
            let now = eff.clock().await;
            let delay = if after_rollback { UNINTERESTING_RETRY_AFTER_ROLLBACK } else { UNINTERESTING_RETRY };
            if state.demote_to_maintenance(peer, conn_id, "uninteresting", now + delay, &eff).await {
                state.regulate_peers(&eff).await;
            }
        }
        PeerSelectionMsg::Promote { peer, conn_id } => {
            let now = eff.clock().await;
            state.try_promote(peer, conn_id, now, &eff).await;
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
    let ledger_entries = ledger.registered_relay_candidates().await;
    let ledger_entries = match ledger_entries {
        Ok(entries) => entries,
        Err(error) => {
            warn!(protocols::peer_selection::ledger::CANDIDATES_FAILED, error = error.to_string());
            return reschedule_check(state, eff).await;
        }
    };
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
