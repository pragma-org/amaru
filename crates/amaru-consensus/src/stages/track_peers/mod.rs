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
    collections::{BTreeMap, BTreeSet},
    mem::take,
    time::Duration,
};

use amaru_kernel::{
    BlockHeader, BlockHeight, Epoch, EraHistory, EraName, IsHeader, ORIGIN_HASH, Peer, Point, Slot, Tip,
    from_cbor_no_leftovers, num::CheckedSub,
};
use amaru_metrics::consensus::ConsensusMetrics;
use amaru_observability::{TraceContext, debug, debug_record, debug_span, error};
use amaru_ouroboros::ConnectionId;
use amaru_ouroboros_traits::Nonces;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    metrics_effects::{Metrics, MetricsOps},
    store_effects::Store,
};
use amaru_pure_stage::{Effects, Instant, OrTerminateWith, ScheduleId, StageRef};
use tracing::Instrument;

use super::peer_selection::PeerSelectionMsg;
use crate::{
    effects::{Ledger, LedgerOps, VolatileTipEffect},
    errors::{ConsensusError, InvalidHeaderParentData, InvalidHeaderPoint},
    stages::select_chain::PerfHeaderForwardOutcome,
};

/// Poll interval while headers are deferred on applied ledger height.
pub const HEIGHT_RECHECK_INTERVAL: Duration = Duration::from_millis(200);

/// Stage that tracks chainsync sessions from whom we receive headers.
///
/// Sessions are keyed by [`ConnectionId`]. On `Initialize` a session is recorded as connecting;
/// after `IntersectFound` tips are tracked. `Terminated` (from the connection stage when chainsync
/// ends or the connection dies) purges all per-connection state, including deferred headers.
///
/// For each established session it keeps the current and highest advertised tip, validates
/// incoming headers for protocol conformance and Praos rules, stores new headers, and notifies
/// `downstream` of new tips. Misbehaving peers are reported to `peer_selection` as adversarial.
///
/// # Construction
///
/// [`TrackPeers::new`] takes an [`EraHistory`], stage refs for peer selection and downstream,
/// `max_peer_lead` (how many block heights a header may lead the applied ledger tip before
/// RequestNext is withheld), and `max_epoch` (latest known stake-distribution epoch; updated by
/// [`TrackPeersMsg::StakeDistUpdated`]).
///
/// # Messages
///
/// - **`FromUpstream`**: ChainSync initiator results (`Initialize`, intersect, roll forward/backward).
/// - **`StakeDistUpdated(epoch)`**: ledger has a new stake distribution; set `max_epoch` and recheck
///   deferred headers.
/// - **`RecheckLedgerHeight`**: self-scheduled wake for deferred ledger-height / clock-skew work;
///   recheck deferred headers against ledger height / time / stake epoch.
///
/// # Roll-forward path
///
/// 1. Decode the header (Conway only; failure → adversarial).
/// 2. If this peer already has deferred work, queue the header as `DeferReason::FollowUp`
///    (pipelined trailers) and return without validating yet.
/// 3. Compare `header.block_height() - max_peer_lead` to a cached applied ledger height
///    ([`VolatileTipEffect`], refreshed at most every 500 ms). If the header is too far ahead,
///    queue `DeferReason::LedgerHeight` **without** sending `RequestNext` and return.
/// 4. Otherwise send `RequestNext` early (pipelining), then `TrackPeers::try_roll_forward`.
///
/// `TrackPeers::try_roll_forward` runs protocol checks (parent, consecutive height, monotonic
/// slot, clock skew) and Praos validation via ledger `validate_header`. On success it advances
/// the peer tip, stores the header if new, and notifies `downstream` with [`NewTip`]. If
/// `RequestNext` was not already sent (e.g. after a height defer is released), it is sent after
/// success.
///
/// # Deferral
///
/// Headers that cannot be finished yet are kept in `deferred` with a reason:
///
/// - **LedgerHeight** — header height exceeds applied ledger by more than `max_peer_lead`.
///   `RequestNext` is not sent until the ledger catches up. Arms a single coalesced
///   `RecheckLedgerHeight` timer (poll interval [`HEIGHT_RECHECK_INTERVAL`]).
/// - **StakeDistribution** — pool stake for the header's epoch is not yet known (at most one
///   epoch ahead of `max_epoch`). `RequestNext` may already have been pipelined before validation
///   failed. Woken by [`TrackPeersMsg::StakeDistUpdated`] (no self-timer).
/// - **ClockSkew** — header slot is at most two slots in the future. Contributes the header onset
///   as the next recheck deadline; `RequestNext` may already have been sent.
/// - **FollowUp** — further headers arrived while the peer was already deferred. Held until
///   earlier deferred items for that peer clear; no `RequestNext` from this path.
///
/// Far-ahead stake (more than one epoch beyond `max_epoch`), far-future slots (more than two),
/// and other validation failures are adversarial (peer removed and `peer_selection` notified).
///
/// # Recheck
///
/// Time-based deferred work (ledger height + clock skew) shares **one** outstanding
/// `RecheckLedgerHeight` schedule (`recheck_timer`). The earliest deadline among deferred items
/// wins; a later arm does not replace an earlier timer; an earlier arm cancels and replaces a
/// later one. After each recheck, the timer is re-armed only if height/clock work remains.
///
/// `TrackPeers::recheck_deferred` walks `deferred` in order. A peer stays blocked while any
/// earlier deferred item for that peer is still blocked (so FollowUps wait on prior
/// LedgerHeight / stake / clock items). When an item is ready, it is re-run through
/// `TrackPeers::try_roll_forward`. If re-running an item fails validation (adversarial), the
/// connection is purged and its remaining deferred items are dropped.
///
/// # Effects and sends
///
/// - **Effects**: `VolatileTipEffect`, ledger `validate_header`, store load / has / store, `clock`,
///   `schedule_at` / `cancel_schedule` (coalesced deferred recheck). Trace context is attached via
///   [`TraceContext`] on ledger and store operations.
/// - **Sends**: per-peer `RequestNext` / `Done`; `Adversarial(peer, TraceContext)` to peer selection;
///   [`NewTip`] to downstream when a new header is stored.
///
/// Logging: INFO (init / intersect / rollback), DEBUG (store / defer), TRACE (roll-forward entry),
/// ERROR (failures), WARN (unknown intersect).
///
/// Exercised via the simulation harness in `test_setup.rs` and tests in `tests.rs`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackPeers {
    era_history: EraHistory,
    upstream: BTreeMap<ConnectionId, PerPeer>,
    peer_selection: StageRef<PeerSelectionMsg>,
    downstream: StageRef<NewTip>,
    max_peer_lead: u64,
    ledger_applied_block_height: BlockHeight,
    ledger_last_checked_at: Instant,
    max_epoch: Epoch,
    deferred: Vec<DeferredHeader>,
    /// Single outstanding self-schedule for height/clock deferred rechecks.
    recheck_timer: Option<ScheduleId>,
}

/// Per-connection tip tracking for a chainsync session.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum PerPeer {
    /// Session started (`Initialize`); intersection not yet established.
    Connecting { peer: Peer },
    /// Intersection established; tips are tracked.
    Established { peer: Peer, current: Tip, highest: Tip },
}

impl PerPeer {
    fn established(&self) -> Option<(&Tip, &Tip)> {
        match self {
            PerPeer::Established { current, highest, .. } => Some((current, highest)),
            PerPeer::Connecting { .. } => None,
        }
    }

    fn established_mut(&mut self) -> Option<(&mut Tip, &mut Tip)> {
        match self {
            PerPeer::Established { current, highest, .. } => Some((current, highest)),
            PerPeer::Connecting { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum DeferReason {
    /// Wait until the ledger has reached at least this applied block height before asking the peer for more.
    LedgerHeight { min_height: BlockHeight, header: BlockHeader, tip: Tip, variant: EraName },
    /// The header's validation requires a stake distribution that is not yet available; hold the
    /// data needed to re-validate and store once it arrives (via StakeDistUpdated).
    StakeDistribution { epoch: Epoch, header: BlockHeader, tip: Tip, variant: EraName, rn_sent: bool },
    /// Slot onset is in the near future (≤ 2s according to slot time); defer validation until
    /// local time reaches it. Carries data to re-process later.
    ClockSkew { min_time: Instant, header: BlockHeader, tip: Tip, variant: EraName, rn_sent: bool },
    /// A follow-up header that was received after a previous header was deferred.
    FollowUp { header: BlockHeader, tip: Tip, variant: EraName },
}

/// A header (or request) that was deferred. The reason indicates what is blocking and what data
/// (if any) must be retained to resume.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DeferredHeader {
    peer: Peer,
    conn_id: ConnectionId,
    handler: StageRef<chainsync::InitiatorMessage>,
    reason: DeferReason,
    trace_context: TraceContext,
    /// When the header was first received from upstream, retained across deferrals so the forward
    /// duration downstream is measured from the original ingress time.
    received_at: Instant,
}

impl PartialEq for DeferredHeader {
    fn eq(&self, other: &Self) -> bool {
        // `received_at` is a performance timestamp used only to measure durations downstream; it does
        // not define the identity of the deferred header, so it is excluded from equality.
        self.peer == other.peer
            && self.conn_id == other.conn_id
            && self.handler == other.handler
            && self.reason == other.reason
            && self.trace_context == other.trace_context
    }
}

struct RollForwardArgs {
    peer: Peer,
    conn_id: ConnectionId,
    sent_request_next: bool,
    handler: StageRef<chainsync::InitiatorMessage>,
    variant: EraName,
    header: BlockHeader,
    tip: Tip,
    trace_context: TraceContext,
    /// When the header was first received from upstream.
    received_at: Instant,
}

impl From<DeferredHeader> for RollForwardArgs {
    fn from(dh: DeferredHeader) -> RollForwardArgs {
        let DeferredHeader { peer, conn_id, handler, reason, trace_context, received_at } = dh;
        match reason {
            DeferReason::LedgerHeight { header, tip, variant, .. } => RollForwardArgs {
                peer,
                conn_id,
                sent_request_next: false,
                handler,
                variant,
                header,
                tip,
                trace_context,
                received_at,
            },
            DeferReason::StakeDistribution { header, tip, variant, rn_sent, .. } => RollForwardArgs {
                peer,
                conn_id,
                sent_request_next: rn_sent,
                handler,
                variant,
                header,
                tip,
                trace_context,
                received_at,
            },
            DeferReason::ClockSkew { header, tip, variant, rn_sent, .. } => RollForwardArgs {
                peer,
                conn_id,
                sent_request_next: rn_sent,
                handler,
                variant,
                header,
                tip,
                trace_context,
                received_at,
            },
            DeferReason::FollowUp { header, tip, variant } => RollForwardArgs {
                peer,
                conn_id,
                sent_request_next: false,
                handler,
                variant,
                header,
                tip,
                trace_context,
                received_at,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TrackPeersMsg {
    FromUpstream(ChainSyncInitiatorMsg),
    /// A new stake distribution is available; recheck any headers deferred for stake dist.
    StakeDistUpdated(Epoch),
    /// Self-scheduled message to check if ledger height has advanced enough for deferred headers.
    RecheckLedgerHeight,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NewTip {
    pub peer: Peer,
    pub tip: Tip,
    pub parent: Point,
    pub trace_context: TraceContext,
    /// When this header was received, so the downstream stage can measure the forward duration.
    pub received_at: Instant,
}

impl PartialEq for NewTip {
    fn eq(&self, other: &Self) -> bool {
        // `received_at` is a performance timestamp used only to measure durations downstream; it does
        // not define the identity of the message, so it is excluded from equality.
        self.peer == other.peer
            && self.tip == other.tip
            && self.parent == other.parent
            && self.trace_context == other.trace_context
    }
}

impl NewTip {
    pub fn new(peer: Peer, tip: Tip, parent: Point) -> Self {
        NewTip {
            peer,
            tip,
            parent,
            trace_context: Default::default(),
            received_at: Instant::at_offset(Duration::ZERO, Duration::ZERO),
        }
    }
}

pub async fn stage(mut state: TrackPeers, msg: TrackPeersMsg, eff: Effects<TrackPeersMsg>) -> TrackPeers {
    match msg {
        TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg { peer, conn_id, handler, msg }) => {
            state.handle_from_upstream(peer, conn_id, handler, msg, eff).await;
        }
        TrackPeersMsg::StakeDistUpdated(max_epoch) => {
            state.max_epoch = max_epoch;
            state.recheck_deferred(&eff).await;
        }
        TrackPeersMsg::RecheckLedgerHeight => {
            state.recheck_timer = None;
            state.recheck_deferred(&eff).await;
        }
    }
    state
}

impl TrackPeers {
    pub fn new(
        era_history: EraHistory,
        peer_selection: StageRef<PeerSelectionMsg>,
        downstream: StageRef<NewTip>,
        max_peer_lead: u64,
        max_epoch: Epoch,
    ) -> Self {
        Self {
            era_history,
            upstream: BTreeMap::new(),
            peer_selection,
            downstream,
            max_peer_lead,
            deferred: Vec::new(),
            ledger_applied_block_height: BlockHeight::from(0),
            ledger_last_checked_at: Instant::at_offset(Duration::ZERO, Duration::ZERO),
            max_epoch,
            recheck_timer: None,
        }
    }

    /// Insert or replace an established session's tips. For use in tests.
    #[cfg(test)]
    pub fn insert_peer(&mut self, peer: Peer, conn_id: ConnectionId, current: Tip, highest: Tip) {
        self.upstream.insert(conn_id, PerPeer::Established { peer, current, highest });
    }

    /// Record a connecting session. For use in tests.
    #[cfg(test)]
    pub fn record_connecting(&mut self, peer: Peer, conn_id: ConnectionId) {
        self.upstream.insert(conn_id, PerPeer::Connecting { peer });
    }

    /// Push a deferred FollowUp for tests (so purge can be exercised without full roll-forward setup).
    #[cfg(test)]
    pub fn push_deferred_for_tests(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        handler: StageRef<chainsync::InitiatorMessage>,
        header: BlockHeader,
        tip: Tip,
    ) {
        self.deferred.push(DeferredHeader {
            peer,
            conn_id,
            handler,
            reason: DeferReason::FollowUp { header, tip, variant: EraName::Conway },
            trace_context: TraceContext::default(),
            received_at: Instant::at_offset(Duration::ZERO, Duration::ZERO),
        });
    }

    /// Validate an incoming header for protocol conformance.
    ///
    /// The received `tip` is the highest advertised tip for the peer as part of the RollForward message.
    ///
    /// If the store already holds evolved nonces for this header, it went through full validation
    /// before (nonces are only stored together with a validated header), so the header is skipped
    /// and `None` is returned. Otherwise the header is validated and the point of its parent is
    /// returned, together with the nonces to store alongside it.
    ///
    /// Note: a header can already sit in the chain store without carrying any nonces, as is the
    /// case for headers imported during bootstrap. Those still need to be validated.
    #[expect(clippy::too_many_arguments)]
    async fn validate_header(
        &mut self,
        peer: &Peer,
        conn_id: ConnectionId,
        variant: EraName,
        header: &BlockHeader,
        tip: Tip,
        ledger: &Ledger,
        store: &Store,
        current_time: Instant,
    ) -> Result<Option<(Point, Nonces)>, ConsensusError> {
        let era_name = self.era_history.slot_to_era_tag(header.slot())?;
        if era_name != variant {
            return Err(ConsensusError::EraNameMismatch { from_raw_header: variant, from_slot: era_name });
        }

        let Some((current, _highest)) = self.upstream.get(&conn_id).and_then(PerPeer::established) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        if header.parent_hash().unwrap_or(ORIGIN_HASH) != current.hash() {
            return Err(ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer.clone(),
                forwarded: header.point(),
                actual: header.parent_hash(),
                expected: current.point(),
            })));
        }
        if header.block_height() != current.block_height() + 1 {
            return Err(ConsensusError::InvalidHeaderHeight {
                actual: header.block_height(),
                expected: current.block_height() + 1,
            });
        }

        // this is the point up to which the upstream peer has validated its best chain, which
        // can be less advanced than the currently transmitted header
        let highest = tip.point();

        // check that slot time progresses monotonically
        if header.slot() <= current.slot() {
            return Err(ConsensusError::InvalidHeaderPoint(Box::new(InvalidHeaderPoint {
                actual: header.point(),
                parent: current.point(),
                highest,
            })));
        }

        // Clock skew using current time from clock (converted to slot via era params / slot length),
        // instead of per_peer.current.
        let elapsed = current_time.duration_since_global_epoch();
        let curr_slot = self.era_history.relative_time_to_slot(elapsed).unwrap_or_else(|_| Slot::from(0));
        if header.slot() > curr_slot {
            let delta_slots = header.slot() - curr_slot;
            if delta_slots > 2 {
                return Err(ConsensusError::InvalidHeaderPoint(Box::new(InvalidHeaderPoint {
                    actual: header.point(),
                    parent: current.point(),
                    highest,
                })));
            }
            return Err(ConsensusError::HeaderSlotInNearFuture(header.slot()));
        }

        // Stored nonces are the durable proof that a header was fully validated: they are only
        // written together with the header they belong to, once it passed all the Praos checks.
        if store.get_nonces(&header.hash()).await.is_some() {
            return Ok(None);
        }
        let nonces = ledger
            .validate_header(header)
            .await
            .map_err(|e| ConsensusError::InvalidHeader(header.point(), Box::new(e)))?;
        Ok(Some((current.point(), nonces)))
    }

    async fn roll_forward(&mut self, conn_id: ConnectionId, header: &BlockHeader, tip: Tip) {
        let Some((current, highest)) = self.upstream.get_mut(&conn_id).and_then(PerPeer::established_mut) else {
            return;
        };
        *current = header.tip();
        *highest = tip;
    }

    async fn roll_backward(
        &mut self,
        peer: &Peer,
        conn_id: ConnectionId,
        current: Point,
        tip: Tip,
        store: &Store,
    ) -> Result<(), ConsensusError> {
        let Some(current_tip) = store.load_tip(&current.hash()).await else {
            return Err(ConsensusError::UnknownPoint(current.hash()));
        };
        let Some((current_ref, highest_ref)) = self.upstream.get_mut(&conn_id).and_then(PerPeer::established_mut)
        else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        *current_ref = current_tip;
        *highest_ref = tip;
        Ok(())
    }

    /// Remove all per-connection state for a chainsync session. Idempotent.
    fn purge_connection(&mut self, conn_id: ConnectionId) {
        self.upstream.remove(&conn_id);
        self.deferred.retain(|d| d.conn_id != conn_id);
    }

    /// Try to defer this header validation due to missing stake distribution.
    /// Returns true if deferred (and not adversarial).
    /// Rejects (returns false to let caller do adversarial) if the missing dist is >1 epoch ahead.
    fn try_defer_for_stake(&mut self, args: &RollForwardArgs, error: &ConsensusError) -> Option<DeferredHeader> {
        let validate_error = error.as_invalid_header()?;
        let target = validate_error.missing_stake_distribution()?;

        // target more than one epoch ahead of known stake dists → adversarial; otherwise defer.
        // Use checked_sub so target < max_epoch does not panic (treat as defer / retry).
        if target.checked_sub(&self.max_epoch).is_some_and(|d| d > *Epoch::ONE) {
            return None;
        }
        Some(DeferredHeader {
            peer: args.peer.clone(),
            conn_id: args.conn_id,
            handler: args.handler.clone(),
            reason: DeferReason::StakeDistribution {
                epoch: target,
                header: args.header.clone(),
                tip: args.tip,
                variant: args.variant,
                rn_sent: args.sent_request_next,
            },
            trace_context: args.trace_context.clone(),
            received_at: args.received_at,
        })
    }

    /// Try to defer this header validation due to the slot being slightly in the future (clock skew).
    /// Returns true if deferred. The caller must arm the coalesced recheck timer after enqueueing.
    async fn try_defer_for_clock_skew(
        &mut self,
        args: &RollForwardArgs,
        error: &ConsensusError,
        eff: &Effects<TrackPeersMsg>,
    ) -> Option<DeferredHeader> {
        if !matches!(error, ConsensusError::HeaderSlotInNearFuture(_)) {
            return None;
        }
        // compute accurate wait using current clock and header onset from era; last clock check was before validation calculations
        let now = eff.clock().await;
        let elapsed = now.duration_since_global_epoch();
        let onset = self.era_history.slot_to_relative_time_unchecked_horizon(args.header.slot()).unwrap_or_default();
        let wait = onset.saturating_sub(elapsed);
        Some(DeferredHeader {
            peer: args.peer.clone(),
            conn_id: args.conn_id,
            handler: args.handler.clone(),
            reason: DeferReason::ClockSkew {
                min_time: now + wait,
                header: args.header.clone(),
                tip: args.tip,
                variant: args.variant,
                rn_sent: args.sent_request_next,
            },
            trace_context: args.trace_context.clone(),
            received_at: args.received_at,
        })
    }

    /// Earliest instant at which height- or clock-deferred work should be rechecked.
    fn next_recheck_at(&self, now: Instant) -> Option<Instant> {
        self.deferred
            .iter()
            .filter_map(|d| match &d.reason {
                DeferReason::LedgerHeight { .. } => Some(now + HEIGHT_RECHECK_INTERVAL),
                DeferReason::ClockSkew { min_time, .. } => Some(*min_time),
                DeferReason::StakeDistribution { .. } | DeferReason::FollowUp { .. } => None,
            })
            .min()
    }

    /// Ensure at most one outstanding `RecheckLedgerHeight` for time-based deferred work.
    async fn ensure_recheck_armed(&mut self, eff: &Effects<TrackPeersMsg>) {
        let needs_timer = self
            .deferred
            .iter()
            .any(|d| matches!(d.reason, DeferReason::LedgerHeight { .. } | DeferReason::ClockSkew { .. }));
        if !needs_timer {
            if let Some(id) = self.recheck_timer.take() {
                eff.cancel_schedule(id).await;
            }
            return;
        }
        let now = eff.clock().await;
        let Some(when) = self.next_recheck_at(now) else {
            return;
        };
        match self.recheck_timer {
            Some(id) if id.time() <= when => {}
            Some(id) => {
                eff.cancel_schedule(id).await;
                let id = eff.schedule_at(TrackPeersMsg::RecheckLedgerHeight, when).await;
                self.recheck_timer = Some(id);
            }
            None => {
                let id = eff.schedule_at(TrackPeersMsg::RecheckLedgerHeight, when).await;
                self.recheck_timer = Some(id);
            }
        }
    }

    fn is_deferred(&self, conn_id: ConnectionId) -> bool {
        self.deferred.iter().any(|d| d.conn_id == conn_id)
    }

    /// Try to execute a roll forward from a peer. Preconditions like maximum distance from applied
    /// ledger height have already been checked. Stake distribution unavailability leads to deferral,
    /// so that this method can be called again later with the same inputs. `Ok(())` is also
    /// returned in case the peer was removed due to an unrecoverable error.
    async fn try_roll_forward(
        &mut self,
        args: RollForwardArgs,
        eff: &Effects<TrackPeersMsg>,
        now: Instant,
    ) -> Result<(), DeferredHeader> {
        let RollForwardArgs { peer, conn_id, variant, header, tip, trace_context, .. } = &args;

        let ledger = Ledger::new(eff.clone()).with_trace_context(trace_context);
        let store = Store::new(eff.clone()).with_trace_context(trace_context);

        let result = self.validate_header(peer, *conn_id, *variant, header, *tip, &ledger, &store, now).await;
        let validated = match result {
            Ok(validated) => validated,
            Err(error) => {
                if let Some(dh) = self.try_defer_for_stake(&args, &error) {
                    return Err(dh);
                } else if let Some(dh) = self.try_defer_for_clock_skew(&args, &error, eff).await {
                    return Err(dh);
                }
                error!(
                    consensus::perf::header::LIFECYCLE,
                    peer = peer.clone(),
                    header_hash = header.hash(),
                    error = %error,
                    outcome = PerfHeaderForwardOutcome::InvalidHeader.as_str()
                );
                record_header_rejected(eff, PerfHeaderForwardOutcome::InvalidHeader).await;

                self.purge_connection(*conn_id);
                eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(args.peer, args.trace_context)).await;
                return Ok(());
            }
        };
        self.roll_forward(*conn_id, header, *tip).await;

        // now we can destructure to consume the pieces
        let RollForwardArgs { peer, header, tip, sent_request_next, handler, trace_context, received_at, .. } = args;
        let header_tip = header.tip();
        let current = header_tip.point();
        match validated {
            None => {
                tracing::debug!(%peer, %current, highest = %tip.point(), "roll forward, header already stored");
                debug!(
                    consensus::perf::header::LIFECYCLE,
                    peer = peer.clone(),
                    header_hash = current.hash(),
                    outcome = PerfHeaderForwardOutcome::DuplicateHeader.as_str()
                );
                record_header_rejected(eff, PerfHeaderForwardOutcome::DuplicateHeader).await;
            }
            Some((parent, nonces)) => {
                // the header and its nonces are stored atomically, so that stored nonces always
                // denote a fully validated header, and follow-up headers can be validated
                store
                    .store_validated_header(&header, &nonces)
                    .or_terminate_with(eff, async |e| {
                        let error = ConsensusError::StoreHeaderFailed(header.hash(), e);
                        error!(
                            consensus::perf::header::LIFECYCLE,
                            peer = peer.clone(),
                            header_hash = current.hash(),
                            error = %error,
                            outcome = PerfHeaderForwardOutcome::StoreHeaderError.as_str()
                        );
                        record_header_rejected(eff, PerfHeaderForwardOutcome::StoreHeaderError).await;
                    })
                    .await;
                tracing::debug!(%peer, %current, highest = %tip.point(), "roll forward with new header");
                eff.send(&self.downstream, NewTip { peer, tip: header_tip, parent, trace_context, received_at }).await;
            }
        }

        if !sent_request_next {
            eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
        }
        Ok(())
    }

    async fn handle_from_upstream(
        &mut self,
        peer: Peer,
        conn_id: ConnectionId,
        handler: StageRef<chainsync::InitiatorMessage>,
        msg: chainsync::InitiatorResult,
        eff: Effects<TrackPeersMsg>,
    ) {
        use amaru_protocols::chainsync::InitiatorResult::*;
        match msg {
            Initialize => {
                let had_state =
                    self.upstream.contains_key(&conn_id) || self.deferred.iter().any(|d| d.conn_id == conn_id);
                if had_state {
                    tracing::warn!(
                        %peer,
                        %conn_id,
                        "unexpected re-initialize of an active chainsync session; purging prior state"
                    );
                    self.purge_connection(conn_id);
                }
                tracing::info!(%peer, %conn_id, "initializing chainsync");
                self.upstream.insert(conn_id, PerPeer::Connecting { peer });
            }
            Terminated => {
                tracing::info!(%peer, %conn_id, "chainsync terminated, purging connection state");
                self.purge_connection(conn_id);
            }
            IntersectFound(current, tip) => {
                let current_tip = Store::new(eff.clone()).load_tip(&current.hash()).await;
                let Some(current_tip) = current_tip else {
                    tracing::warn!(%peer, %current, tip = %tip.point(), reason = "peer sent unknown intersection point", "stopping chainsync");
                    eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                    return;
                };
                tracing::info!(%peer, %conn_id, %current, highest = %tip.point(), "intersect found");
                self.upstream.insert(conn_id, PerPeer::Established { peer, current: current_tip, highest: tip });
            }
            IntersectNotFound(tip) => {
                tracing::info!(%peer, highest = %tip.point(), reason = "intersect not found", "stopping chainsync");
                eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                self.purge_connection(conn_id);
            }
            RollForward(header_content, tip) => {
                let peer_clone = peer.clone();
                let span = debug_span!(root, consensus::roll_forward::PROCESS, tip = tip, peer = peer_clone,);
                let trace_context: TraceContext = (&span).into();
                async {
                    tracing::trace!(%peer, variant = header_content.variant.as_str(), highest = %tip.point(), "roll forward");

                    let variant = header_content.variant;
                    let probe = decode_header(header_content, &peer);
                    let header = match probe {
                        Ok(h) => h,
                        Err(error) => {
                            self.purge_connection(conn_id);
                            error!(
                                consensus::perf::header::LIFECYCLE,
                                peer = peer.clone(),
                                error = %error,
                                outcome = PerfHeaderForwardOutcome::UndecodableHeader.as_str()
                            );
                            record_header_rejected(&eff, PerfHeaderForwardOutcome::UndecodableHeader).await;
                            eff.send(
                                &self.peer_selection,
                                PeerSelectionMsg::Adversarial(peer, trace_context.clone()),
                            )
                                .await;
                            return;
                        }
                    };
                    debug_record!(consensus::roll_forward::PROCESS, header_hash = header.hash());

                    let now = eff.clock().await;

                    if self.is_deferred(conn_id) {
                        self.deferred.push(DeferredHeader {
                            peer,
                            conn_id,
                            handler,
                            reason: DeferReason::FollowUp { header, tip, variant },
                            trace_context,
                            received_at: now,
                        });
                        return;
                    }

                    let header_height = header.block_height();
                    let limit = header_height - self.max_peer_lead;
                    // maybe update ledger applied block height (rate-limited to 500ms or initial)
                    if limit > self.ledger_applied_block_height
                        && (now.saturating_since(self.ledger_last_checked_at) > Duration::from_millis(500)
                        || self.ledger_applied_block_height == BlockHeight::from(0))
                    {
                        self.ledger_last_checked_at = now;
                        self.ledger_applied_block_height = eff.external(VolatileTipEffect).await.block_height();
                    }

                    let ledger_height = self.ledger_applied_block_height;
                    if ledger_height < limit {
                        tracing::debug!(%peer, %header_height, %ledger_height, %limit, "track_peers.defer_request_next");
                        self.deferred.push(DeferredHeader {
                            peer: peer.clone(),
                            conn_id,
                            handler: handler.clone(),
                            reason: DeferReason::LedgerHeight {
                                header: header.clone(),
                                tip,
                                variant,
                                min_height: limit,
                            },
                            trace_context,
                            received_at: now,
                        });
                        self.ensure_recheck_armed(&eff).await;
                        return;
                    }

                    eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
                    let args = RollForwardArgs {
                        peer,
                        conn_id,
                        sent_request_next: true,
                        handler,
                        variant,
                        header,
                        tip,
                        trace_context,
                        received_at: now,
                    };
                    if let Err(dh) = self.try_roll_forward(args, &eff, now).await {
                        self.deferred.push(dh);
                        self.ensure_recheck_armed(&eff).await;
                    }
                }
                    .instrument(span)
                    .await
            }
            RollBackward(current, tip) => {
                tracing::info!(%peer, %current, highest = %tip.point(), "roll backward");
                let peer_clone = peer.clone();
                let span = debug_span!(
                    root,
                    consensus::rollback::PROCESS,
                    current = %current,
                    peer = %peer_clone,
                    tip = %tip,
                    header_hash = tip.hash(),
                );
                let trace_context: TraceContext = (&span).into();
                async {
                    eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;

                    let store = Store::new(eff.clone()).with_trace_context(&trace_context);
                    if let Err(error) = self.roll_backward(&peer, conn_id, current, tip, &store).await {
                        tracing::error!(%error, %peer, "chain_sync.roll_backward.failed");
                        self.purge_connection(conn_id);
                        eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer, trace_context)).await;
                    }
                }
                .instrument(span)
                .await
            }
        }
    }

    async fn recheck_deferred(&mut self, eff: &Effects<TrackPeersMsg>) {
        let curr_height = eff.external(VolatileTipEffect).await.block_height();
        self.ledger_applied_block_height = curr_height;

        let current_time = eff.clock().await;

        // try_roll_forward may purge a connection, which removes its entries from
        // self.deferred. Iterating over a taken copy keeps that reentrant mutation
        // from invalidating the iteration; entries already taken out are beyond the
        // purge's reach, so they are skipped explicitly.
        let mut blocked = BTreeSet::new();
        for d in take(&mut self.deferred) {
            let conn_id = d.conn_id;
            if !self.upstream.contains_key(&conn_id) {
                continue;
            }
            let defer = blocked.contains(&conn_id)
                || match &d.reason {
                    DeferReason::LedgerHeight { min_height, .. } => curr_height < *min_height,
                    DeferReason::StakeDistribution { epoch, .. } => self.max_epoch < *epoch,
                    DeferReason::ClockSkew { min_time, .. } => current_time < *min_time,
                    DeferReason::FollowUp { .. } => false,
                };
            if defer {
                blocked.insert(conn_id);
                self.deferred.push(d);
                continue;
            }

            if let Err(dh) = self.try_roll_forward(d.into(), eff, current_time).await {
                // the connection must still be marked as blocked
                blocked.insert(conn_id);
                self.deferred.push(dh);
            }
        }
        self.ensure_recheck_armed(eff).await;
    }
}

pub fn decode_header(raw_header: HeaderContent, peer: &Peer) -> Result<BlockHeader, ConsensusError> {
    let span = debug_span!(consensus::header::DECODE, peer = peer);
    let _guard = span.enter();
    // need to list all the variants supported by the current Amaru implementation
    if !matches!(raw_header.variant, EraName::Conway) {
        return Err(ConsensusError::InvalidHeaderVariant(raw_header.variant));
    }
    from_cbor_no_leftovers(&raw_header.cbor)
        .map_err(|reason| ConsensusError::CannotDecodeHeader { header: raw_header.cbor, reason: reason.to_string() })
}

/// Record the `header_lifecycle` metric for a header rejected on reception.
/// Such a header carries no lifecycle durations, only its `outcome`.
async fn record_header_rejected<T: amaru_pure_stage::SendData + Sync>(
    eff: &Effects<T>,
    outcome: PerfHeaderForwardOutcome,
) {
    Metrics::new(eff)
        .record(
            ConsensusMetrics::HeaderLifecycle {
                outcome: outcome.as_str().to_string(),
                block_fetch_wait_micros: None,
                block_fetch_micros: None,
                forward_micros: None,
            }
            .into(),
        )
        .await;
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
