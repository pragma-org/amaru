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
use amaru_observability::{TraceContext, debug_record, debug_span};
use amaru_ouroboros::praos::header::AssertHeaderError;
use amaru_ouroboros_traits::has_stake_distribution::GetPoolError;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    store_effects::Store,
};
use amaru_pure_stage::{Effects, Instant, OrTerminateWith, ScheduleId, StageRef};
use tracing::Instrument;

use super::peer_selection::PeerSelectionMsg;
use crate::{
    effects::{Ledger, LedgerOps, VolatileTipEffect},
    errors::{ConsensusError, InvalidHeaderParentData, InvalidHeaderPoint},
    validate_header::ValidateHeaderError,
};

/// Poll interval while headers are deferred on applied ledger height.
pub const HEIGHT_RECHECK_INTERVAL: Duration = Duration::from_millis(200);

/// Stage that tracks peers from whom we receive headers over ChainSync.
///
/// For each peer it keeps the current and highest advertised tip, validates incoming headers for
/// protocol conformance and Praos rules, stores new headers, and notifies `downstream` of new
/// tips. Misbehaving peers are reported to `peer_selection` as adversarial.
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
/// `TrackPeers::try_roll_forward`.
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
    upstream: BTreeMap<Peer, PerPeer>,
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PerPeer {
    current: Tip,
    highest: Tip,
}

#[derive(Default, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    #[default]
    Placeholder,
}

/// A header (or request) that was deferred. The reason indicates what is blocking and what data
/// (if any) must be retained to resume.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct DeferredHeader {
    peer: Peer,
    handler: StageRef<chainsync::InitiatorMessage>,
    reason: DeferReason,
    trace_context: TraceContext,
}
impl Default for DeferredHeader {
    fn default() -> Self {
        Self {
            peer: Peer::new(""),
            handler: StageRef::blackhole(),
            reason: DeferReason::default(),
            trace_context: Default::default(),
        }
    }
}

struct RollForwardArgs {
    peer: Peer,
    sent_request_next: bool,
    handler: StageRef<chainsync::InitiatorMessage>,
    variant: EraName,
    header: BlockHeader,
    tip: Tip,
    trace_context: TraceContext,
}

impl From<DeferredHeader> for RollForwardArgs {
    fn from(dh: DeferredHeader) -> RollForwardArgs {
        let DeferredHeader { peer, handler, reason, trace_context } = dh;
        #[expect(clippy::panic)]
        match reason {
            DeferReason::LedgerHeight { header, tip, variant, .. } => {
                RollForwardArgs { peer, sent_request_next: false, handler, variant, header, tip, trace_context }
            }
            DeferReason::StakeDistribution { header, tip, variant, rn_sent, .. } => {
                RollForwardArgs { peer, sent_request_next: rn_sent, handler, variant, header, tip, trace_context }
            }
            DeferReason::ClockSkew { header, tip, variant, rn_sent, .. } => {
                RollForwardArgs { peer, sent_request_next: rn_sent, handler, variant, header, tip, trace_context }
            }
            DeferReason::FollowUp { header, tip, variant } => {
                RollForwardArgs { peer, sent_request_next: false, handler, variant, header, tip, trace_context }
            }
            DeferReason::Placeholder => panic!("cannot convert Placeholder to RollForwardArgs"),
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewTip {
    pub tip: Tip,
    pub parent: Point,
    pub trace_context: TraceContext,
}

pub async fn stage(mut state: TrackPeers, msg: TrackPeersMsg, eff: Effects<TrackPeersMsg>) -> TrackPeers {
    match msg {
        TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg { peer, conn_id: _, handler, msg }) => {
            state.handle_from_upstream(peer, handler, msg, eff).await;
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

    /// Insert or replace a peer's current and highest tip. For use in tests.
    #[cfg(test)]
    pub fn insert_peer(&mut self, peer: Peer, current: Tip, highest: Tip) {
        self.upstream.insert(peer, PerPeer { current, highest });
    }

    /// Validate an incoming header for protocol conformance and store it in the chain store.
    ///
    /// The received `tip` is the highest advertised tip for the peer as part of the RollForward message.
    async fn validate_header(
        &mut self,
        peer: &Peer,
        variant: EraName,
        header: &BlockHeader,
        tip: Tip,
        ledger: &Ledger,
        current_time: Instant,
    ) -> Result<Point, ConsensusError> {
        let era_name = self.era_history.slot_to_era_tag(header.slot())?;
        if era_name != variant {
            return Err(ConsensusError::EraNameMismatch { from_raw_header: variant, from_slot: era_name });
        }

        let Some(per_peer) = self.upstream.get(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        if header.parent_hash().unwrap_or(ORIGIN_HASH) != per_peer.current.hash() {
            return Err(ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer.clone(),
                forwarded: header.point(),
                actual: header.parent_hash(),
                expected: per_peer.current.point(),
            })));
        }
        if header.block_height() != per_peer.current.block_height() + 1 {
            return Err(ConsensusError::InvalidHeaderHeight {
                actual: header.block_height(),
                expected: per_peer.current.block_height() + 1,
            });
        }

        // this is the point up to which the upstream peer has validated its best chain, which
        // can be less advanced than the currently transmitted header
        let highest = tip.point();

        // check that slot time progresses monotonically
        if header.slot() <= per_peer.current.slot() {
            return Err(ConsensusError::InvalidHeaderPoint(Box::new(InvalidHeaderPoint {
                actual: header.point(),
                parent: per_peer.current.point(),
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
                    parent: per_peer.current.point(),
                    highest,
                })));
            }
            return Err(ConsensusError::HeaderSlotInNearFuture(header.slot()));
        }

        ledger.validate_header(header).await.map_err(|e| ConsensusError::InvalidHeader(header.point(), Box::new(e)))?;
        Ok(per_peer.current.point())
    }

    async fn roll_forward(&mut self, peer: &Peer, header: &BlockHeader, tip: Tip) {
        let Some(per_peer) = self.upstream.get_mut(peer) else {
            return;
        };
        per_peer.current = header.tip();
        per_peer.highest = tip;
    }

    async fn maybe_store_header(&mut self, header: BlockHeader, store: &Store) -> Result<bool, ConsensusError> {
        let hash = header.hash();
        if store.has_header(&hash).await {
            Ok(false)
        } else {
            store.store_header(&header).await.map_err(|e| ConsensusError::StoreHeaderFailed(hash, e))?;
            Ok(true)
        }
    }

    async fn roll_backward(
        &mut self,
        peer: &Peer,
        current: Point,
        tip: Tip,
        store: &Store,
    ) -> Result<(), ConsensusError> {
        let Some(current_tip) = store.load_tip(&current.hash()).await else {
            return Err(ConsensusError::UnknownPoint(current.hash()));
        };
        let Some(per_peer) = self.upstream.get_mut(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        per_peer.current = current_tip;
        per_peer.highest = tip;
        Ok(())
    }

    /// Try to defer this header validation due to missing stake distribution.
    /// Returns true if deferred (and not adversarial).
    /// Rejects (returns false to let caller do adversarial) if the missing dist is >1 epoch ahead.
    fn try_defer_for_stake(&mut self, args: &RollForwardArgs, error: &ConsensusError) -> Option<DeferredHeader> {
        let Some(ValidateHeaderError::Assert(AssertHeaderError::PoolError(
            GetPoolError::StakeDistributionNotAvailable(_, Some(target)),
        ))) = error.as_invalid_header()
        else {
            return None;
        };

        // target more than one epoch ahead of known stake dists → adversarial; otherwise defer.
        // Use checked_sub so target < max_epoch does not panic (treat as defer / retry).
        if target.checked_sub(&self.max_epoch).is_some_and(|d| d > *Epoch::ONE) {
            return None;
        }
        Some(DeferredHeader {
            peer: args.peer.clone(),
            handler: args.handler.clone(),
            reason: DeferReason::StakeDistribution {
                epoch: *target,
                header: args.header.clone(),
                tip: args.tip,
                variant: args.variant,
                rn_sent: args.sent_request_next,
            },
            trace_context: args.trace_context.clone(),
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
            handler: args.handler.clone(),
            reason: DeferReason::ClockSkew {
                min_time: now + wait,
                header: args.header.clone(),
                tip: args.tip,
                variant: args.variant,
                rn_sent: args.sent_request_next,
            },
            trace_context: args.trace_context.clone(),
        })
    }

    /// Earliest instant at which height- or clock-deferred work should be rechecked.
    fn next_recheck_at(&self, now: Instant) -> Option<Instant> {
        self.deferred
            .iter()
            .filter_map(|d| match &d.reason {
                DeferReason::LedgerHeight { .. } => Some(now + HEIGHT_RECHECK_INTERVAL),
                DeferReason::ClockSkew { min_time, .. } => Some(*min_time),
                DeferReason::StakeDistribution { .. } | DeferReason::FollowUp { .. } | DeferReason::Placeholder => None,
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

    fn is_deferred(&self, peer: &Peer) -> bool {
        self.deferred.iter().any(|d| d.peer == *peer)
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
        let RollForwardArgs { peer, variant, header, tip, trace_context, .. } = &args;

        let ledger = Ledger::new(eff.clone()).with_trace_context(trace_context);
        let store = Store::new(eff.clone()).with_trace_context(trace_context);

        let result = self.validate_header(peer, *variant, header, *tip, &ledger, now).await;
        let parent = match result {
            Ok(parent) => parent,
            Err(error) => {
                if let Some(dh) = self.try_defer_for_stake(&args, &error) {
                    return Err(dh);
                } else if let Some(dh) = self.try_defer_for_clock_skew(&args, &error, eff).await {
                    return Err(dh);
                }
                tracing::error!(%error, %peer, "chain_sync.validate_header.failed");
                self.upstream.remove(peer);
                eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(args.peer, args.trace_context)).await;
                return Ok(());
            }
        };
        // at this point the evolved nonces have been stored and follow-up headers can be validated
        self.roll_forward(peer, header, *tip).await;

        // now we can destructure to consume the pieces
        let RollForwardArgs { peer, header, tip, sent_request_next, handler, trace_context, .. } = args;
        let header_tip = header.tip();
        let current = header_tip.point();
        let new = self
            .maybe_store_header(header, &store)
            .or_terminate_with(eff, async |error| {
                tracing::error!(%error, %peer, "chain_sync.store_header.failed");
            })
            .await;
        if new {
            tracing::debug!(%peer, %current, highest = %tip.point(), "roll forward with new header");
            eff.send(&self.downstream, NewTip { tip: header_tip, parent, trace_context }).await;
        } else {
            tracing::debug!(%peer, %current, highest = %tip.point(), "roll forward, header already stored");
        }

        if !sent_request_next {
            eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
        }
        Ok(())
    }

    async fn handle_from_upstream(
        &mut self,
        peer: Peer,
        handler: StageRef<chainsync::InitiatorMessage>,
        msg: chainsync::InitiatorResult,
        eff: Effects<TrackPeersMsg>,
    ) {
        use amaru_protocols::chainsync::InitiatorResult::*;
        match msg {
            Initialize => {
                // FIXME record this connection and create a mechanism for removing upon disconnect
                tracing::info!(%peer,"initializing chainsync");
            }
            IntersectFound(current, tip) => {
                let current_tip = Store::new(eff.clone()).load_tip(&current.hash()).await;
                let Some(current_tip) = current_tip else {
                    tracing::warn!(%peer, %current, tip = %tip.point(), reason = "peer sent unknown intersection point", "stopping chainsync");
                    eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                    return;
                };
                tracing::info!(%peer, %current, highest = %tip.point(), "intersect found");
                self.upstream.insert(peer, PerPeer { current: current_tip, highest: tip });
            }
            IntersectNotFound(tip) => {
                tracing::info!(%peer, highest = %tip.point(), reason = "intersect not found", "stopping chainsync");
                eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                self.upstream.remove(&peer);
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
                            tracing::error!(%error, %peer, "chain_sync.decode_header.failed");
                            self.upstream.remove(&peer);
                            eff.send(
                                &self.peer_selection,
                                PeerSelectionMsg::Adversarial(peer, trace_context.clone()),
                            )
                            .await;
                            return;
                        }
                    };
                    debug_record!(consensus::roll_forward::PROCESS, header_hash = header.hash());

                    if self.is_deferred(&peer) {
                        self.deferred.push(DeferredHeader {
                            peer,
                            handler,
                            reason: DeferReason::FollowUp { header, tip, variant },
                            trace_context,
                        });
                        return;
                    }

                    let now = eff.clock().await;

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
                            handler: handler.clone(),
                            reason: DeferReason::LedgerHeight {
                                header: header.clone(),
                                tip,
                                variant,
                                min_height: limit,
                            },
                            trace_context,
                        });
                        self.ensure_recheck_armed(&eff).await;
                        return;
                    }

                    eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
                    let args = RollForwardArgs {
                        peer,
                        sent_request_next: true,
                        handler,
                        variant,
                        header,
                        tip,
                        trace_context,
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
                    if let Err(error) = self.roll_backward(&peer, current, tip, &store).await {
                        tracing::error!(%error, %peer, "chain_sync.roll_backward.failed");
                        self.upstream.remove(&peer);
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

        let mut blocked = BTreeSet::new();
        let mut pos = 0;
        for idx in 0..self.deferred.len() {
            let d = take(&mut self.deferred[idx]);
            let peer = &d.peer;
            let defer = blocked.contains(peer)
                || match &d.reason {
                    DeferReason::LedgerHeight { min_height, .. } => curr_height < *min_height,
                    DeferReason::StakeDistribution { epoch, .. } => self.max_epoch < *epoch,
                    DeferReason::ClockSkew { min_time, .. } => current_time < *min_time,
                    DeferReason::FollowUp { .. } => false,
                    DeferReason::Placeholder => false,
                };
            if defer {
                blocked.insert(d.peer.clone());
                self.deferred[pos] = d;
                pos += 1;
                continue;
            }

            if let Err(dh) = self.try_roll_forward(d.into(), eff, current_time).await {
                self.deferred[pos] = dh;
                pos += 1;
            }
        }
        self.deferred.truncate(pos);
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

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
