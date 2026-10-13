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

//! Outbound peer selection.
//!
//! This stage keeps intent (whom we want, at what local use, and which deadlines are pending).
//! Live bearers, applied local use, dial failures, and intersection-not-found marks live in the
//! performance resource. A single timeout wakes the stage at the earlier of one second and the
//! next stored deadline. The wake copies a peer view only when the resource generation has moved,
//! and runs a full round when that view changed, a deadline is due, or thirty seconds have passed
//! since the last full round. An eviction deadline every five minutes enqueues one bounded
//! retention batch and does not wait for the worker to finish it. A mark for a live Using bearer
//! sets Maintenance until its deadline. A bearer that is cold on both sides closes at once when
//! it is not duplex, and thirty seconds after both sides go cold when it is.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Display},
    time::Duration,
};

use amaru_kernel::{BlockHeight, Peer, PeerCandidate};
use amaru_observability::{CarriedHeader, Instrument, TraceContext, debug, debug_span, info, trace, warn};
use amaru_ouroboros::{ConnectionDirection, ConnectionId, ObservedAt};
use amaru_protocols::{connection::MiniProtocolUse, manager::ManagerMessage};
use amaru_pure_stage::{Effects, Instant, StageRef};

pub use crate::performance::{PeerMix, PeerMixParseError};
use crate::{
    effects::{GenerateRandomSeed, Ledger, LedgerOps, ResolvePeerCandidate, ResolvePeerCandidateResult},
    performance::{
        ChurnRank, DialOutcome, PeerView, Performance, SelectOutboundParams, SelectUsing, UninterestingMark,
        ViewConnection,
    },
};

const STATIC_PEER_BAN_PERIOD: Duration = Duration::from_secs(10);
/// Backoff after a failed Host/SRV lookup before that candidate may be picked again.
const RESOLUTION_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Caught-up churn interval before fuzz (Haskell default).
const CHURN_INTERVAL_BASE: Duration = Duration::from_secs(3300);
/// Extra delay drawn uniformly from `0..=CHURN_INTERVAL_FUZZ`.
const CHURN_INTERVAL_FUZZ: Duration = Duration::from_secs(600);
/// Fraction of Using peers to demote each cycle (at least one).
const CHURN_FRACTION_PERCENT: usize = 20;
/// After clean churn, the bearer stays; do not re-promote for this long.
pub(crate) const CHURN_REPROMOTE_DELAY: Duration = Duration::from_secs(10);
/// Retry Using after no intersection (not hostility).
pub(crate) const UNINTERESTING_RETRY: Duration = Duration::from_secs(120);
/// Retry Using after a rollback past the intersection.
const UNINTERESTING_RETRY_AFTER_ROLLBACK: Duration = Duration::from_secs(180);
/// After a connect failure or a lost dial, do not dial that peer again until this elapses.
/// Starting a dial, adopting it, or seeing the bearer close does not set this hold-off.
const DIAL_HOLDOFF: Duration = Duration::from_secs(2);
/// Upper bound on how often the timeout fires when nothing is due sooner.
const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// A full round runs at least this often, even when the view is unchanged.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// How often one bounded retention batch is enqueued.
const EVICTION_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Do not repeat `SetLocalUse` for one bearer more often than this.
const LOCAL_USE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// How long a duplex bearer stays up after both the local intent and the remote side are cold.
pub(crate) const REMOTE_COLD_DWELL: Duration = Duration::from_secs(30);
/// A dial with no outcome after the connect timeout plus this grace is treated as lost.
const DIAL_LOST_GRACE: Duration = Duration::from_secs(8);
/// Matches [`amaru_protocols::manager::ManagerConfig`]'s default connect timeout.
const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

fn churn_interval(seed: [u8; 32]) -> Duration {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&seed[0..8]);
    let fuzz_secs = u64::from_le_bytes(bytes) % (CHURN_INTERVAL_FUZZ.as_secs() + 1);
    CHURN_INTERVAL_BASE + Duration::from_secs(fuzz_secs)
}

fn observed_instant(at: ObservedAt) -> Instant {
    Instant::at_offset(at.elapsed, at.global_epoch_offset)
}

/// How many manager commands one round may send.
struct SendBudget {
    left: usize,
}

impl SendBudget {
    fn new(upstream: usize, downstream: usize) -> Self {
        Self { left: upstream.saturating_add(downstream) }
    }

    async fn send(
        &mut self,
        eff: &Effects<PeerSelectionMsg>,
        manager: &StageRef<ManagerMessage>,
        msg: ManagerMessage,
    ) -> bool {
        if self.left == 0 {
            return false;
        }
        eff.send(manager, msg).await;
        self.left -= 1;
        true
    }
}

/// Desired bearer. `wanted` is intent; `applied` is the last local use seen in a view.
/// `remote_use` is the derived remote temperature from that view.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct DesiredBearer {
    id: ConnectionId,
    full_duplex: bool,
    wanted: MiniProtocolUse,
    applied: MiniProtocolUse,
    remote_use: MiniProtocolUse,
    local_use_sent_at: Option<Instant>,
    /// When a bearer that is cold on both sides may be closed.
    remote_cold_until: Option<Instant>,
    /// The remote side has run an initiator protocol on this bearer at least once.
    #[serde(default)]
    remote_was_hot: bool,
}

impl DesiredBearer {
    fn adopt(conn: &ViewConnection, wanted: MiniProtocolUse) -> Self {
        Self {
            id: conn.conn_id,
            full_duplex: conn.full_duplex,
            wanted,
            applied: conn.local_use,
            remote_use: conn.remote_use,
            local_use_sent_at: None,
            remote_cold_until: None,
            remote_was_hot: false,
        }
    }

    fn observe(&mut self, conn: &ViewConnection) {
        self.full_duplex = conn.full_duplex;
        self.applied = conn.local_use;
        self.remote_use = conn.remote_use;
    }

    fn set_wanted(&mut self, wanted: MiniProtocolUse) {
        self.wanted = wanted;
        if wanted != MiniProtocolUse::None {
            self.remote_cold_until = None;
        }
    }
}

impl Display for DesiredBearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Connection(id={}, duplex={}, use={})", self.id.as_u64(), self.full_duplex, self.wanted.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum OutboundIntent {
    Dialing { since: Instant, candidate: PeerCandidate },
    Connected(DesiredBearer),
}

/// Peer selection stage for the Amaru consensus node.
///
/// Outbound candidate sources and the admin peer-mix formula live in the performance resource.
/// This stage stores desired bearers, in-flight dials, and deadlines. It asks the resource for a
/// view and for outbound inputs; sampling and churn ranking run here, after those copies return.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerSelection {
    target_upstream_peers: usize,
    target_downstream_peers: usize,
    manager: StageRef<ManagerMessage>,
    peer_removal_cooldown: Duration,
    /// Outbound TCP connect budget. A dial with no outcome after this plus the lost-dial grace is lost.
    connection_timeout: Duration,
    cooldowns: Cooldowns,
    inbound_peers: BTreeMap<Peer, DesiredBearer>,
    outbound_peers: BTreeMap<Peer, OutboundIntent>,
    /// Host/SRV candidates with an in-flight [`ResolvePeerCandidate`] (not yet dialled).
    pending_resolve: BTreeSet<PeerCandidate>,
    /// Host/SRV candidates currently bound to an outbound [`Peer`] (re-resolved after unbind).
    bound: BTreeMap<PeerCandidate, Peer>,
    /// Failed Host/SRV lookups that must not be re-selected until the stored instant.
    resolve_backoff: BTreeMap<PeerCandidate, Instant>,
    /// Candidates not dialled again until this instant.
    dial_holdoff: BTreeMap<PeerCandidate, Instant>,
    /// Latest close instant already applied for a peer. A later view that repeats that close does
    /// not cancel a dial started afterwards; only a strictly newer close does.
    applied_close: BTreeMap<Peer, Instant>,
    /// Cooling bearers whose `Disconnect` was sent and that are still in the latest view.
    disconnect_sent: BTreeSet<ConnectionId>,
    /// Next churn. `None` until [`PeerSelectionMsg::Initialize`].
    next_churn_at: Option<Instant>,
    /// Peers demoted from Using that must not be re-promoted until this instant.
    /// A due entry is dropped at the start of the next full round, even when the peer is not promoted.
    demoted_until: BTreeMap<Peer, Instant>,
    /// Resource generation last copied into a round. The next query passes this as `since`.
    seen_generation: u64,
    /// Next forced full round. `None` until the first full round.
    next_sweep_at: Option<Instant>,
    /// Next retention batch. `None` until [`PeerSelectionMsg::Initialize`] or the first full round.
    next_evict_at: Option<Instant>,
}

impl PartialEq for PeerSelection {
    fn eq(&self, other: &Self) -> bool {
        self.target_upstream_peers == other.target_upstream_peers
            && self.target_downstream_peers == other.target_downstream_peers
            && self.manager == other.manager
            && self.peer_removal_cooldown == other.peer_removal_cooldown
            && self.connection_timeout == other.connection_timeout
            && self.cooldowns == other.cooldowns
            && self.inbound_peers == other.inbound_peers
            && self.outbound_peers == other.outbound_peers
            && self.pending_resolve == other.pending_resolve
            && self.bound == other.bound
            && self.resolve_backoff == other.resolve_backoff
            && self.dial_holdoff == other.dial_holdoff
            && self.applied_close == other.applied_close
            && self.disconnect_sent == other.disconnect_sent
            && self.next_churn_at == other.next_churn_at
            && self.demoted_until == other.demoted_until
            && self.seen_generation == other.seen_generation
            && self.next_sweep_at == other.next_sweep_at
            && self.next_evict_at == other.next_evict_at
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PeerSelectionMsg {
    /// Connect to initial peers, start the ledger check, and arm the timeout.
    Initialize,
    /// The peer has performed an adversarial action, such as sending invalid blocks or headers.
    ///
    /// The first report while no ban is active removes and bans the peer. A repeat report during
    /// that ban is logged and ignored. Refill waits for the next tick.
    Adversarial(Peer, TraceContext<CarriedHeader>),
    /// Manually add a peer, mostly for testing.
    AddPeer(Peer),
    /// DNS result for a selected bootstrap [`amaru_kernel::PeerCandidate`] (at most one [`Peer`]).
    Resolved(ResolvePeerCandidateResult),
    /// Wake from the single timeout.
    Tick,
}

impl PeerSelectionMsg {
    /// Shortcut for creating an adversarial message when no trace context is available.
    pub fn adversarial(peer: Peer) -> PeerSelectionMsg {
        PeerSelectionMsg::Adversarial(peer, TraceContext::detached())
    }
}

impl PeerSelection {
    /// Construct connection-selection state only.
    ///
    /// Outbound candidate sources and the peer-mix formula are installed exclusively when
    /// constructing the [`Performance`] resource (`with_peer_sources`).
    pub fn new(
        manager: StageRef<ManagerMessage>,
        target_upstream_peers: usize,
        target_downstream_peers: usize,
        peer_removal_cooldown: Duration,
    ) -> Self {
        Self {
            target_upstream_peers,
            target_downstream_peers,
            manager,
            peer_removal_cooldown,
            connection_timeout: DEFAULT_CONNECTION_TIMEOUT,
            cooldowns: Cooldowns::default(),
            inbound_peers: BTreeMap::new(),
            outbound_peers: BTreeMap::new(),
            pending_resolve: BTreeSet::new(),
            bound: BTreeMap::new(),
            resolve_backoff: BTreeMap::new(),
            dial_holdoff: BTreeMap::new(),
            applied_close: BTreeMap::new(),
            disconnect_sent: BTreeSet::new(),
            next_churn_at: None,
            demoted_until: BTreeMap::new(),
            seen_generation: 0,
            next_sweep_at: None,
            next_evict_at: None,
        }
    }

    /// Align lost-dial detection with the manager's connect timeout.
    pub fn with_connection_timeout(mut self, connection_timeout: Duration) -> Self {
        self.connection_timeout = connection_timeout;
        self
    }
}

impl PeerSelection {
    fn dial_lost_after(&self) -> Duration {
        self.connection_timeout + DIAL_LOST_GRACE
    }

    fn budget(&self) -> SendBudget {
        SendBudget::new(self.target_upstream_peers, self.target_downstream_peers)
    }

    async fn ban_peer(&mut self, peer: Peer, eff: &Effects<PeerSelectionMsg>) {
        let is_static = eff.external(Performance::is_static_peer(peer)).await;
        let now = eff.clock().await;

        if let Some(bearer) = self.inbound_peers.remove(&peer) {
            warn!(
                protocols::peer_selection::peer::REMOVED,
                peer,
                direction = "inbound",
                peer_state = bearer.to_string(),
                is_static
            );
        }
        if let Some(intent) = self.outbound_peers.remove(&peer) {
            warn!(
                protocols::peer_selection::peer::REMOVED,
                peer,
                direction = "outbound",
                peer_state = format!("{intent:?}"),
                is_static
            );
            self.unbind_peer(&peer);
            self.demoted_until.remove(&peer);
        }

        eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
        eff.external(Performance::peer_adversarial(peer, now)).await;
        let ban_period = if is_static { STATIC_PEER_BAN_PERIOD } else { self.peer_removal_cooldown };
        self.cooldowns.cooldown_until.insert(peer, now + ban_period);
    }

    fn unbind_peer(&mut self, peer: &Peer) {
        self.bound.retain(|_, bound| bound != peer);
    }

    async fn start_dial(
        &mut self,
        candidate: PeerCandidate,
        origin: crate::performance::PeerSource,
        peer: Peer,
        now: Instant,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) -> bool {
        let manager = self.manager.clone();
        if !budget.send(eff, &manager, ManagerMessage::AddPeer(peer)).await {
            return false;
        }
        eff.external(Performance::note_dial(origin, candidate.clone(), peer, now)).await;
        if candidate.needs_resolution() {
            self.bound.insert(candidate, peer);
        }
        info!(protocols::peer_selection::peer::ADDED, peer, was_banned = false);
        self.outbound_peers.insert(peer, OutboundIntent::Dialing { since: now, candidate: PeerCandidate::from(peer) });
        true
    }

    fn hold_dial_many(&mut self, peer: Peer, also: impl IntoIterator<Item = PeerCandidate>, now: Instant) {
        let until = now + DIAL_HOLDOFF;
        self.push_dial_holdoff(PeerCandidate::from(peer), until);
        for candidate in also {
            self.push_dial_holdoff(candidate, until);
        }
    }

    fn push_dial_holdoff(&mut self, candidate: PeerCandidate, until: Instant) {
        let slot = self.dial_holdoff.entry(candidate).or_insert(until);
        if *slot < until {
            *slot = until;
        }
    }

    /// Drop hold-off for this peer, the name it was dialled under, and any bound alias.
    fn clear_dial_holdoff(&mut self, peer: Peer, dialing: &PeerCandidate) {
        self.dial_holdoff.remove(&PeerCandidate::from(peer));
        self.dial_holdoff.remove(dialing);
        for candidate in self.related_candidates(peer) {
            self.dial_holdoff.remove(&candidate);
        }
    }

    /// First sight of a close at or after `since`. A close already applied does not count again
    /// unless it is strictly newer.
    fn close_instant_to_apply(&self, view: Option<&PeerView>, peer: Peer, since: Instant) -> Option<Instant> {
        let DialOutcome::Closed { at, .. } = view?.closes.get(&peer).copied()?;
        let at = observed_instant(at);
        if self.applied_close.get(&peer).is_some_and(|applied| at <= *applied) {
            return None;
        }
        (at >= since).then_some(at)
    }

    fn related_candidates(&self, peer: Peer) -> Vec<PeerCandidate> {
        self.bound.iter().filter(|(_, bound)| **bound == peer).map(|(candidate, _)| candidate.clone()).collect()
    }

    fn using_occupancy(&self) -> usize {
        self.pending_resolve.len()
            + self
                .outbound_peers
                .values()
                .filter(|state| match state {
                    OutboundIntent::Dialing { .. } => true,
                    OutboundIntent::Connected(bearer) => bearer.wanted == MiniProtocolUse::Diffusion,
                })
                .count()
            + self.inbound_peers.values().filter(|bearer| bearer.wanted == MiniProtocolUse::Diffusion).count()
    }

    fn using_peers(&self) -> Vec<Peer> {
        self.outbound_peers
            .iter()
            .filter_map(|(peer, state)| match state {
                OutboundIntent::Connected(bearer) if bearer.wanted == MiniProtocolUse::Diffusion => Some(*peer),
                OutboundIntent::Dialing { .. } | OutboundIntent::Connected(_) => None,
            })
            .collect()
    }

    fn bearer_mut(&mut self, peer: Peer, conn_id: ConnectionId) -> Option<&mut DesiredBearer> {
        if let Some(OutboundIntent::Connected(bearer)) = self.outbound_peers.get_mut(&peer)
            && bearer.id == conn_id
        {
            return Some(bearer);
        }
        if let Some(bearer) = self.inbound_peers.get_mut(&peer)
            && bearer.id == conn_id
        {
            return Some(bearer);
        }
        None
    }

    #[expect(clippy::too_many_arguments)]
    async fn demote_to_maintenance(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        reason: &'static str,
        until: Instant,
        now: Instant,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) -> bool {
        let Some(bearer) = self.bearer_mut(peer, conn_id) else {
            return false;
        };
        if bearer.wanted != MiniProtocolUse::Diffusion {
            return false;
        }
        let manager = self.manager.clone();
        if !budget
            .send(eff, &manager, ManagerMessage::SetLocalUse { peer, conn_id, local_use: MiniProtocolUse::Maintenance })
            .await
        {
            return false;
        }
        let Some(bearer) = self.bearer_mut(peer, conn_id) else {
            return false;
        };
        bearer.set_wanted(MiniProtocolUse::Maintenance);
        bearer.local_use_sent_at = Some(now);
        self.demoted_until.insert(peer, until);
        info!(protocols::peer_selection::peer::DEMOTED, peer, conn_id = conn_id.as_u64(), reason);
        true
    }

    async fn try_promote(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        now: Instant,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) {
        if self.demoted_until.get(&peer).is_some_and(|until| *until > now) {
            return;
        }
        self.demoted_until.remove(&peer);
        if self.using_occupancy() >= self.target_upstream_peers {
            return;
        }
        let Some(bearer) = self.bearer_mut(peer, conn_id) else {
            return;
        };
        if bearer.wanted == MiniProtocolUse::Diffusion {
            return;
        }
        let manager = self.manager.clone();
        if !budget
            .send(eff, &manager, ManagerMessage::SetLocalUse { peer, conn_id, local_use: MiniProtocolUse::Diffusion })
            .await
        {
            return;
        }
        if let Some(bearer) = self.bearer_mut(peer, conn_id) {
            bearer.set_wanted(MiniProtocolUse::Diffusion);
            bearer.local_use_sent_at = Some(now);
        }
    }

    async fn churn(&mut self, now: Instant, eff: &Effects<PeerSelectionMsg>, budget: &mut SendBudget) {
        let using = self.using_peers();
        if using.is_empty() {
            return;
        }
        let want = (using.len() * CHURN_FRACTION_PERCENT / 100).max(1).min(using.len());
        let ranked = eff.external(Performance::rank_peers_for_churn(using, now)).await;
        let mut demoted = 0;
        for ChurnRank { peer, is_static, .. } in ranked {
            if demoted >= want {
                break;
            }
            if is_static {
                continue;
            }
            let Some(OutboundIntent::Connected(bearer)) = self.outbound_peers.get(&peer) else {
                continue;
            };
            if bearer.wanted != MiniProtocolUse::Diffusion {
                continue;
            }
            let conn_id = bearer.id;
            if self.demote_to_maintenance(peer, conn_id, "churn", now + CHURN_REPROMOTE_DELAY, now, eff, budget).await {
                demoted += 1;
            }
        }
    }

    fn promotable_duplex_inbounds(&self, now: Instant) -> Vec<(Peer, ConnectionId)> {
        self.inbound_peers
            .iter()
            .filter_map(|(peer, bearer)| {
                if bearer.full_duplex
                    && bearer.wanted != MiniProtocolUse::Diffusion
                    && !self.demoted_until.get(peer).is_some_and(|until| *until > now)
                    && !self.cooldowns.is_cooling(peer)
                {
                    Some((*peer, bearer.id))
                } else {
                    None
                }
            })
            .collect()
    }

    async fn promote_duplex_inbounds(
        &mut self,
        now: Instant,
        limit: usize,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) {
        if limit == 0 {
            return;
        }
        let candidates = self.promotable_duplex_inbounds(now);
        let mut promoted = 0;
        for (peer, conn_id) in candidates {
            if promoted >= limit || self.using_occupancy() >= self.target_upstream_peers {
                break;
            }
            let before = self.using_occupancy();
            self.try_promote(peer, conn_id, now, eff, budget).await;
            if self.using_occupancy() > before {
                promoted += 1;
            }
        }
    }

    async fn promote_eligible_maintenance(
        &mut self,
        now: Instant,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) {
        let candidates: Vec<(Peer, ConnectionId)> = self
            .outbound_peers
            .iter()
            .filter_map(|(peer, state)| match state {
                OutboundIntent::Connected(bearer)
                    if bearer.wanted == MiniProtocolUse::Maintenance
                        && !self.demoted_until.get(peer).is_some_and(|until| *until > now) =>
                {
                    Some((*peer, bearer.id))
                }
                OutboundIntent::Dialing { .. } | OutboundIntent::Connected(_) => None,
            })
            .collect();
        for (peer, conn_id) in candidates {
            if self.using_occupancy() >= self.target_upstream_peers {
                break;
            }
            self.try_promote(peer, conn_id, now, eff, budget).await;
        }
    }

    async fn regulate_peers(&mut self, now: Instant, eff: &Effects<PeerSelectionMsg>, budget: &mut SendBudget) {
        self.promote_eligible_maintenance(now, eff, budget).await;
        let occupancy = self.using_occupancy();
        if occupancy >= self.target_upstream_peers {
            self.dial_holdoff.retain(|_, until| *until > now);
            self.resolve_backoff.retain(|_, until| *until > now);
            return;
        }
        let open = self.target_upstream_peers - occupancy;
        let eligible_inbound = self.promotable_duplex_inbounds(now).len();

        let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
        let mut excluded: BTreeSet<PeerCandidate> =
            self.outbound_peers.keys().copied().map(PeerCandidate::from).collect();
        excluded.extend(
            self.inbound_peers
                .iter()
                .filter(|(_, bearer)| bearer.wanted == MiniProtocolUse::Diffusion)
                .map(|(peer, _)| PeerCandidate::from(*peer)),
        );
        for peer in self.cooldowns.cooling_peers() {
            excluded.insert(PeerCandidate::from(peer));
        }
        excluded.extend(self.pending_resolve.iter().cloned());
        excluded.extend(self.bound.keys().cloned());
        self.resolve_backoff.retain(|_, until| *until > now);
        excluded.extend(self.resolve_backoff.keys().cloned());
        self.dial_holdoff.retain(|_, until| *until > now);
        excluded.extend(self.dial_holdoff.keys().cloned());
        let SelectUsing { inbound, outbound } = eff
            .external(Performance::select_outbound(SelectOutboundParams {
                open,
                excluded,
                eligible_inbound,
                seed,
                now,
            }))
            .await;
        self.promote_duplex_inbounds(now, inbound, eff, budget).await;
        for pick in outbound {
            match pick.candidate.as_peer() {
                Some(peer) => {
                    if self.outbound_peers.contains_key(&peer) {
                        continue;
                    }
                    if !self.start_dial(pick.candidate, pick.origin, peer, now, eff, budget).await {
                        break;
                    }
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

    fn expire_lost_dials(&mut self, view: Option<&PeerView>, now: Instant) {
        let lost_after = self.dial_lost_after();
        let peers: Vec<(Peer, Instant, PeerCandidate)> = self
            .outbound_peers
            .iter()
            .filter_map(|(peer, intent)| match intent {
                OutboundIntent::Dialing { since, candidate } => Some((*peer, *since, candidate.clone())),
                OutboundIntent::Connected(_) => None,
            })
            .collect();
        for (peer, since, candidate) in peers {
            let established = view.and_then(|view| {
                view.connections
                    .iter()
                    .find(|conn| conn.peer == peer && conn.direction == ConnectionDirection::Outbound)
            });
            if let Some(conn) = established {
                self.outbound_peers
                    .insert(peer, OutboundIntent::Connected(DesiredBearer::adopt(conn, MiniProtocolUse::Diffusion)));
                continue;
            }
            // An inbound bearer for this peer means the dial is no longer needed. A failure
            // timestamp must not hold the live connection off; only the dial is dropped.
            let inbound_live = view.is_some_and(|view| {
                view.connections.iter().any(|conn| conn.peer == peer && conn.direction == ConnectionDirection::Inbound)
            });
            if inbound_live {
                self.outbound_peers.remove(&peer);
                self.unbind_peer(&peer);
                continue;
            }
            // A close at or after the dial is the bearer ending, not a failed connect.
            // Apply each close once, clear any hold-off, and let the round refill.
            // A dial started after that application is cancelled only by a strictly newer close.
            if let Some(at) = self.close_instant_to_apply(view, peer, since) {
                self.applied_close.insert(peer, at);
                self.clear_dial_holdoff(peer, &candidate);
                self.outbound_peers.remove(&peer);
                self.unbind_peer(&peer);
                self.demoted_until.remove(&peer);
                continue;
            }
            let failed = view
                .is_some_and(|view| view.connect_failures.get(&peer).is_some_and(|at| observed_instant(*at) >= since));
            let lost = now >= since + lost_after;
            if failed || lost {
                let names = self.related_candidates(peer);
                self.outbound_peers.remove(&peer);
                self.unbind_peer(&peer);
                self.hold_dial_many(peer, names, now);
            }
        }
    }

    /// Returns whether a cooling bearer still needs `Disconnect` after this round.
    async fn reconcile(
        &mut self,
        view: &PeerView,
        now: Instant,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) -> bool {
        let mut deferred = false;
        self.expire_lost_dials(Some(view), now);
        let live: BTreeSet<ConnectionId> = view.connections.iter().map(|conn| conn.conn_id).collect();
        self.disconnect_sent.retain(|id| live.contains(id));
        let dropped_in: Vec<(Peer, ConnectionId)> = self
            .inbound_peers
            .iter()
            .filter(|(_, bearer)| !live.contains(&bearer.id))
            .map(|(peer, bearer)| (*peer, bearer.id))
            .collect();
        for (peer, conn_id) in dropped_in {
            self.inbound_peers.remove(&peer);
            self.demoted_until.remove(&peer);
            let _span = debug_span!(
                protocols::peer_selection::peer::DISCONNECTED,
                peer,
                conn_id = conn_id.as_u64(),
                direction = ConnectionDirection::Inbound,
            )
            .entered();
        }
        let dropped_out: Vec<(Peer, ConnectionId)> = self
            .outbound_peers
            .iter()
            .filter_map(|(peer, intent)| match intent {
                OutboundIntent::Connected(bearer) if !live.contains(&bearer.id) => Some((*peer, bearer.id)),
                OutboundIntent::Dialing { .. } | OutboundIntent::Connected(_) => None,
            })
            .collect();
        for (peer, conn_id) in dropped_out {
            self.outbound_peers.remove(&peer);
            self.unbind_peer(&peer);
            self.demoted_until.remove(&peer);
            let _span = debug_span!(
                protocols::peer_selection::peer::DISCONNECTED,
                peer,
                conn_id = conn_id.as_u64(),
                direction = ConnectionDirection::Outbound,
            )
            .entered();
        }

        for conn in &view.connections {
            if self.cooldowns.is_cooling(&conn.peer) {
                if self.disconnect_sent.contains(&conn.conn_id) {
                    continue;
                }
                let manager = self.manager.clone();
                if budget.send(eff, &manager, ManagerMessage::Disconnect(conn.peer, conn.conn_id)).await {
                    self.disconnect_sent.insert(conn.conn_id);
                    self.drop_bearer(conn.peer, conn.conn_id);
                } else {
                    deferred = true;
                }
                continue;
            }
            match conn.direction {
                ConnectionDirection::Inbound => self.adopt_inbound(conn),
                ConnectionDirection::Outbound => self.adopt_outbound(conn),
            }
            self.consider_remote_cold(conn.peer, conn.conn_id, now, eff, budget).await;
        }
        self.apply_uninteresting(&view.uninteresting, now);
        self.sync_local_use(now, eff, budget).await;
        deferred
    }

    /// Demote a live bearer named by a mark. A missing bearer, or one already in Maintenance, is ignored.
    fn apply_uninteresting(&mut self, marks: &[UninterestingMark], now: Instant) {
        for mark in marks {
            let delay = if mark.after_rollback { UNINTERESTING_RETRY_AFTER_ROLLBACK } else { UNINTERESTING_RETRY };
            {
                let Some(bearer) = self.bearer_mut(mark.peer, mark.conn_id) else {
                    continue;
                };
                if bearer.wanted != MiniProtocolUse::Diffusion {
                    continue;
                }
                bearer.set_wanted(MiniProtocolUse::Maintenance);
            }
            let peer = mark.peer;
            let conn_id = mark.conn_id;
            self.demoted_until.insert(peer, now + delay);
            let reason = "uninteresting";
            info!(protocols::peer_selection::peer::DEMOTED, peer, conn_id = conn_id.as_u64(), reason);
        }
    }

    fn drop_bearer(&mut self, peer: Peer, conn_id: ConnectionId) {
        if self.inbound_peers.get(&peer).is_some_and(|bearer| bearer.id == conn_id) {
            self.inbound_peers.remove(&peer);
            self.demoted_until.remove(&peer);
        }
        if let Some(OutboundIntent::Connected(bearer)) = self.outbound_peers.get(&peer)
            && bearer.id == conn_id
        {
            self.outbound_peers.remove(&peer);
            self.unbind_peer(&peer);
            self.demoted_until.remove(&peer);
        }
    }

    fn adopt_inbound(&mut self, conn: &ViewConnection) {
        match self.inbound_peers.get_mut(&conn.peer) {
            Some(bearer) if bearer.id == conn.conn_id => bearer.observe(conn),
            _ => {
                let peer = conn.peer;
                let conn_id = conn.conn_id;
                self.inbound_peers.insert(peer, DesiredBearer::adopt(conn, MiniProtocolUse::None));
                let _span = debug_span!(
                    protocols::peer_selection::peer::CONNECTED,
                    peer,
                    conn_id = conn_id.as_u64(),
                    direction = ConnectionDirection::Inbound,
                    full_duplex = conn.full_duplex,
                    local_use = conn.local_use.as_str(),
                )
                .entered();
            }
        }
    }

    fn adopt_outbound(&mut self, conn: &ViewConnection) {
        if let Some(OutboundIntent::Connected(bearer)) = self.outbound_peers.get_mut(&conn.peer)
            && bearer.id == conn.conn_id
        {
            bearer.observe(conn);
            return;
        }
        let peer = conn.peer;
        let conn_id = conn.conn_id;
        self.outbound_peers
            .insert(peer, OutboundIntent::Connected(DesiredBearer::adopt(conn, MiniProtocolUse::Diffusion)));
        let _span = debug_span!(
            protocols::peer_selection::peer::CONNECTED,
            peer,
            conn_id = conn_id.as_u64(),
            direction = ConnectionDirection::Outbound,
            full_duplex = conn.full_duplex,
            local_use = conn.local_use.as_str(),
        )
        .entered();
    }

    async fn sync_local_use(&mut self, now: Instant, eff: &Effects<PeerSelectionMsg>, budget: &mut SendBudget) {
        let mut pending = Vec::new();
        for (peer, bearer) in &self.inbound_peers {
            if local_use_ready(bearer, now) {
                pending.push((*peer, bearer.id, bearer.wanted));
            }
        }
        for (peer, intent) in &self.outbound_peers {
            if let OutboundIntent::Connected(bearer) = intent
                && local_use_ready(bearer, now)
            {
                pending.push((*peer, bearer.id, bearer.wanted));
            }
        }
        for (peer, conn_id, local_use) in pending {
            let manager = self.manager.clone();
            if !budget.send(eff, &manager, ManagerMessage::SetLocalUse { peer, conn_id, local_use }).await {
                break;
            }
            if let Some(bearer) = self.bearer_mut(peer, conn_id) {
                bearer.local_use_sent_at = Some(now);
            }
        }
    }

    /// Close a bearer whose local intent and remote temperature are both [`MiniProtocolUse::None`].
    ///
    /// A bearer that has not yet shown a remote initiator, and every duplex bearer, waits
    /// [`REMOTE_COLD_DWELL`] from the first round that sees both cold. That deadline is not moved
    /// while both stay cold. A one-way bearer that has already been hot closes on this round.
    /// A spent send budget leaves the deadline due so the next round retries with one close.
    async fn consider_remote_cold(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        now: Instant,
        eff: &Effects<PeerSelectionMsg>,
        budget: &mut SendBudget,
    ) {
        let close = {
            let Some(bearer) = self.bearer_mut(peer, conn_id) else {
                return;
            };
            if bearer.remote_use != MiniProtocolUse::None {
                bearer.remote_was_hot = true;
            }
            if bearer.wanted != MiniProtocolUse::None || bearer.remote_use != MiniProtocolUse::None {
                bearer.remote_cold_until = None;
                return;
            }
            // A brand-new one-way inbound has no remote initiator yet. Closing it on the first
            // round drops the bearer before that initiator can open.
            if bearer.full_duplex || !bearer.remote_was_hot {
                let until = *bearer.remote_cold_until.get_or_insert(now + REMOTE_COLD_DWELL);
                until <= now
            } else {
                // Due immediately. A later round must still find it due if this send did not happen.
                if !bearer.remote_cold_until.is_some_and(|until| until <= now) {
                    bearer.remote_cold_until = Some(now);
                }
                true
            }
        };
        if !close {
            return;
        }
        let manager = self.manager.clone();
        if budget.send(eff, &manager, ManagerMessage::Disconnect(peer, conn_id)).await {
            self.drop_bearer(peer, conn_id);
        }
    }

    async fn consider_stored_cold(&mut self, now: Instant, eff: &Effects<PeerSelectionMsg>, budget: &mut SendBudget) {
        let mut bearers = Vec::new();
        for (peer, bearer) in &self.inbound_peers {
            bearers.push((*peer, bearer.id));
        }
        for (peer, intent) in &self.outbound_peers {
            if let OutboundIntent::Connected(bearer) = intent {
                bearers.push((*peer, bearer.id));
            }
        }
        for (peer, conn_id) in bearers {
            self.consider_remote_cold(peer, conn_id, now, eff, budget).await;
        }
    }

    fn remote_cold_deadlines(&self) -> impl Iterator<Item = Instant> + '_ {
        let inbound = self.inbound_peers.values().filter_map(|bearer| bearer.remote_cold_until);
        let outbound = self.outbound_peers.values().filter_map(|intent| match intent {
            OutboundIntent::Connected(bearer) => bearer.remote_cold_until,
            OutboundIntent::Dialing { .. } => None,
        });
        inbound.chain(outbound)
    }

    fn due(&self, now: Instant) -> bool {
        self.cooldowns.cooldown_until.values().any(|until| *until <= now)
            || self.dial_holdoff.values().any(|until| *until <= now)
            || self.resolve_backoff.values().any(|until| *until <= now)
            || self.demoted_until.values().any(|until| *until <= now)
            || self.remote_cold_deadlines().any(|until| until <= now)
            || self.next_churn_at.is_some_and(|at| at <= now)
            || self.next_sweep_at.is_some_and(|at| at <= now)
            || self.next_evict_at.is_some_and(|at| at <= now)
            || self.lost_dial_due(now)
            || self.local_use_due(now)
    }

    fn lost_dial_due(&self, now: Instant) -> bool {
        let lost_after = self.dial_lost_after();
        self.outbound_peers.values().any(|intent| match intent {
            OutboundIntent::Dialing { since, .. } => now >= *since + lost_after,
            OutboundIntent::Connected(_) => false,
        })
    }

    fn local_use_due(&self, now: Instant) -> bool {
        self.inbound_peers.values().any(|bearer| local_use_ready(bearer, now))
            || self.outbound_peers.values().any(|intent| match intent {
                OutboundIntent::Connected(bearer) => local_use_ready(bearer, now),
                OutboundIntent::Dialing { .. } => false,
            })
    }

    fn earliest_future(&self, now: Instant) -> Option<Instant> {
        let mut best: Option<Instant> = None;
        let mut consider = |when: Instant| {
            if when <= now {
                return;
            }
            best = Some(match best {
                Some(current) if current <= when => current,
                _ => when,
            });
        };
        for when in self.cooldowns.cooldown_until.values().copied() {
            consider(when);
        }
        for when in self.dial_holdoff.values().copied() {
            consider(when);
        }
        for when in self.resolve_backoff.values().copied() {
            consider(when);
        }
        for when in self.demoted_until.values().copied() {
            consider(when);
        }
        for when in self.remote_cold_deadlines() {
            consider(when);
        }
        if let Some(when) = self.next_churn_at {
            consider(when);
        }
        if let Some(when) = self.next_sweep_at {
            consider(when);
        }
        if let Some(when) = self.next_evict_at {
            consider(when);
        }
        let lost_after = self.dial_lost_after();
        for intent in self.outbound_peers.values() {
            if let OutboundIntent::Dialing { since, .. } = intent {
                consider(*since + lost_after);
            }
        }
        for bearer in self.inbound_peers.values() {
            if bearer.wanted != bearer.applied
                && let Some(sent) = bearer.local_use_sent_at
            {
                consider(sent + LOCAL_USE_MIN_INTERVAL);
            }
        }
        for intent in self.outbound_peers.values() {
            if let OutboundIntent::Connected(bearer) = intent
                && bearer.wanted != bearer.applied
                && let Some(sent) = bearer.local_use_sent_at
            {
                consider(sent + LOCAL_USE_MIN_INTERVAL);
            }
        }
        best
    }

    fn arm_delay(&self, now: Instant) -> Duration {
        self.earliest_future(now).map(|when| when.saturating_since(now)).unwrap_or(TICK_INTERVAL).min(TICK_INTERVAL)
    }

    async fn arm_timeout(&self, eff: &Effects<PeerSelectionMsg>) {
        let now = eff.clock().await;
        eff.set_timeout(self.arm_delay(now), PeerSelectionMsg::Tick).await;
    }

    async fn on_tick(&mut self, eff: &Effects<PeerSelectionMsg>) {
        let now = eff.clock().await;
        let view = eff.external(Performance::query_peer_view(self.seen_generation)).await;
        let full = view.is_some() || self.due(now);
        trace!(protocols::peer_selection::TICK, full);
        if !full {
            return;
        }
        self.run_round(now, view, eff).await;
    }

    fn lift_due_demotions(&mut self, now: Instant) {
        self.demoted_until.retain(|_, until| *until > now);
    }

    async fn run_round(&mut self, now: Instant, view: Option<PeerView>, eff: &Effects<PeerSelectionMsg>) {
        self.lift_due_demotions(now);
        let mut budget = self.budget();
        if let Some(view) = view.as_ref() {
            let deferred = self.reconcile(view, now, eff, &mut budget).await;
            // A cooling disconnect that did not fit stays outstanding. The next tick reads this
            // view again. Ids already sent are skipped, so the budget reaches the ones still waiting.
            if !deferred {
                self.seen_generation = view.generation;
            }
        } else {
            self.expire_lost_dials(None, now);
            // Generation unchanged means the bearer set and remote temperature are unchanged.
            // A cold deadline that came due still closes from the last observation.
            self.consider_stored_cold(now, eff, &mut budget).await;
        }
        self.cooldowns.lift_due(now);
        self.promote_eligible_maintenance(now, eff, &mut budget).await;
        if self.next_churn_at.is_some_and(|at| at <= now) {
            self.churn(now, eff, &mut budget).await;
            let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
            self.next_churn_at = Some(now + churn_interval(seed));
        }
        self.regulate_peers(now, eff, &mut budget).await;
        // A mark sets desired use during reconcile. If that round's rate limit held the command,
        // a later round whose deadline is this interval still has to send it.
        self.sync_local_use(now, eff, &mut budget).await;
        self.schedule_eviction(now, eff).await;
        self.next_sweep_at = Some(now + SWEEP_INTERVAL);
    }

    async fn schedule_eviction(&mut self, now: Instant, eff: &Effects<PeerSelectionMsg>) {
        if self.next_evict_at.is_some_and(|at| at > now) {
            return;
        }
        if self.next_evict_at.is_some() {
            eff.external(Performance::evict_records(
                now,
                self.eviction_protected_peers(),
                self.eviction_protected_candidates(),
            ))
            .await;
        }
        self.next_evict_at = Some(now + EVICTION_INTERVAL);
    }

    fn eviction_protected_peers(&self) -> BTreeSet<Peer> {
        let mut peers = BTreeSet::new();
        peers.extend(self.cooldowns.cooling_peers());
        peers.extend(self.inbound_peers.keys().copied());
        peers.extend(self.outbound_peers.keys().copied());
        peers.extend(self.bound.values().copied());
        peers
    }

    fn eviction_protected_candidates(&self) -> BTreeSet<PeerCandidate> {
        let mut candidates = BTreeSet::new();
        candidates.extend(self.pending_resolve.iter().cloned());
        candidates.extend(self.bound.keys().cloned());
        for intent in self.outbound_peers.values() {
            if let OutboundIntent::Dialing { candidate, .. } = intent {
                candidates.insert(candidate.clone());
            }
        }
        candidates
    }
}

fn local_use_ready(bearer: &DesiredBearer, now: Instant) -> bool {
    if bearer.wanted == bearer.applied {
        return false;
    }
    match bearer.local_use_sent_at {
        None => true,
        Some(sent) => now >= sent + LOCAL_USE_MIN_INTERVAL,
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Cooldowns {
    cooldown_until: BTreeMap<Peer, Instant>,
}

impl Cooldowns {
    fn is_cooling(&self, peer: &Peer) -> bool {
        self.cooldown_until.contains_key(peer)
    }

    fn cooling_peers(&self) -> impl Iterator<Item = Peer> + '_ {
        self.cooldown_until.keys().copied()
    }

    fn lift_due(&mut self, now: Instant) {
        self.cooldown_until.retain(|_, until| *until > now);
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
            let now = eff.clock().await;
            let mut budget = state.budget();
            state.regulate_peers(now, &eff, &mut budget).await;
            let seed: [u8; 32] = eff.external(GenerateRandomSeed).await;
            state.next_churn_at = Some(now + churn_interval(seed));
            state.next_sweep_at = Some(now + SWEEP_INTERVAL);
            state.next_evict_at = Some(now + EVICTION_INTERVAL);
            // NOTE: no supervision, failure in ledger-check shall tear down the node.
            let ledger_check = eff
                .wire_up(eff.stage("peer-selection/ledger-check", get_ledger_candidates).await, LedgerCheck::new())
                .await;
            eff.send(&ledger_check, ()).await;
        }
        PeerSelectionMsg::Adversarial(peer, trace_context) => {
            if state.cooldowns.is_cooling(&peer) {
                debug!(protocols::peer_selection::peer::ADVERSARIAL_DUPLICATE, peer);
            } else {
                debug!(protocols::peer_selection::peer::ADVERSARIAL, peer);
                let span = debug_span!(parent_context: &trace_context, consensus::peer::BAN, peer);
                state.ban_peer(peer, &eff).instrument(span.into()).await;
            }
        }
        PeerSelectionMsg::AddPeer(peer) => {
            let was_banned = state.cooldowns.cooldown_until.remove(&peer).is_some();
            if !state.outbound_peers.contains_key(&peer) {
                info!(protocols::peer_selection::peer::ADDED, peer, was_banned);
                let now = eff.clock().await;
                eff.send(&state.manager, ManagerMessage::AddPeer(peer)).await;
                state
                    .outbound_peers
                    .insert(peer, OutboundIntent::Dialing { since: now, candidate: PeerCandidate::from(peer) });
            } else {
                info!(protocols::peer_selection::peer::ADD_SKIPPED, peer, reason = "already_added");
            }
        }
        PeerSelectionMsg::Resolved(ResolvePeerCandidateResult { candidate, origin, peer }) => {
            state.pending_resolve.remove(&candidate);
            let Some(peer) = peer else {
                let now = eff.clock().await;
                state.resolve_backoff.insert(candidate, now + RESOLUTION_RETRY_DELAY);
                let mut budget = state.budget();
                state.regulate_peers(now, &eff, &mut budget).await;
                state.arm_timeout(&eff).await;
                return state;
            };
            if state.cooldowns.is_cooling(&peer) || state.outbound_peers.contains_key(&peer) {
                let now = eff.clock().await;
                eff.external(Performance::note_dial(origin, candidate.clone(), peer, now)).await;
                if candidate.needs_resolution() {
                    state.bound.insert(candidate, peer);
                }
            } else {
                info!(
                    protocols::peer_selection::peer::RESOLVED,
                    candidate = candidate.to_string(),
                    origin = origin.as_str(),
                    peer,
                );
                let now = eff.clock().await;
                let mut budget = state.budget();
                state.start_dial(candidate, origin, peer, now, &eff, &mut budget).await;
                state.regulate_peers(now, &eff, &mut budget).await;
            }
        }
        PeerSelectionMsg::Tick => {
            state.on_tick(&eff).await;
        }
    }
    state.arm_timeout(&eff).await;
    state
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct LedgerCheck {
    last_height: BlockHeight,
    cadence: Duration,
    min_height_change: u64,
}

impl LedgerCheck {
    fn new() -> Self {
        Self { last_height: BlockHeight::from(0), cadence: Duration::from_secs(60), min_height_change: 3000 }
    }
}

async fn get_ledger_candidates(state: LedgerCheck, msg: (), eff: Effects<()>) -> LedgerCheck {
    let span =
        debug_span!(protocols::peer_selection::ledger::CHECK_CANDIDATES, last_height = state.last_height.as_u64());
    get_ledger_candidates_inner(state, msg, eff).instrument(span.into()).await
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
    // The write bumps the resource generation. The parent's next tick refills; this child
    // does not send a message.
    eff.external(Performance::set_ledger_candidates(ledger_entries)).await;
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
