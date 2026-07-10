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

mod defer_req_next;

use std::{collections::BTreeMap, time::Duration};

use amaru_kernel::{
    BlockHeader, EraHistory, EraName, IsHeader, ORIGIN_HASH, Peer, Point, Slot, SlotDelta, Tip, from_cbor_no_leftovers,
};
use amaru_observability::trace_span;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    store_effects::Store,
};
use amaru_pure_stage::{Effects, Instant, StageRef};
pub use defer_req_next::DeferReqNextMsg;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::peer_selection::PeerSelectionMsg;
use crate::{
    effects::{Ledger, LedgerOps},
    errors::{ConsensusError, InvalidHeaderParentData, InvalidHeaderPoint},
};

/// Slot of the furthest ledger-applied state: volatile tip if present, otherwise stable tip.
pub(super) async fn ledger_applied_slot<T: amaru_pure_stage::SendData + Sync>(eff: &Effects<T>) -> Slot {
    let ledger = Ledger::new(eff.clone());
    ledger.volatile_tip().await.slot()
}

/// This is the state of the [`stage`] that tracks peers from whom we are receiving headers.
///
/// It maintains the currently communicated tip as well as the highest advertised tip for each peer.
/// With this information, it validates incoming headers for protocol conformance and ensures that
/// they are stored in the chain store. When a new header is stored, its [`Tip`] is sent to the
/// `downstream` stage. The `peer_selection` stage removes misbehaving peers and applies cooldown policy.
///
/// The stage is driven exclusively by `TrackPeersMsg::FromUpstream` (the only variant). All
/// external interaction occurs via `amaru_pure_stage::Effects` (sends, dynamic child `stage`/`wire_up`,
/// `clock`, and `schedule_after`) plus the `Ledger` and `Store` effect abstractions (for
/// `volatile_tip`/`validate_header` and `load_tip`/`has_header`/`store_header`).
///
/// # Construction
/// - Created via [`TrackPeers::new`] with an `EraHistory`, `StageRef`s for peer_selection and
///   downstream, the maximum tolerated slot forecast, and `defer_req_next_poll_ms`.
/// - The `defer_req_next` child ref starts as `StageRef::blackhole()` and is materialized lazily
///   (see below).
///
/// # Message Handling (TrackPeersMsg)
///
/// Only one top-level variant exists:
///
/// - `TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg { peer, conn_id: _, handler, msg })`:
///   delegates to `handle_from_upstream`. `conn_id` is ignored in
///   all paths. The inner `InitiatorResult` cases are:
///
///   - `Initialize`: logs at INFO "initializing chainsync" (mod.rs:282-283). No state change.
///     (Tests: `test_new_peer`, `test_initialize_existing_peer`.)
///
///   - `IntersectFound(current, tip)`: performs `Store::load_tip(current.hash())` (external effect).
///     If missing → WARN + `handler` ← `Done` + early return (no insert).
///     If present → INFO "intersect found" + insert `PerPeer { current: loaded_tip, highest: tip }`.
///     (Tests: `test_intersect_found_*`.)
///
///   - `IntersectNotFound(tip)`: INFO "intersect not found" + `handler` ← `Done` + `upstream.remove(&peer)`.
///     (Tests: `test_intersect_not_found_*`.)
///
///   - `RollForward(header_content, tip)`: TRACE log. Decodes via `decode_header` (only Conway
///     supported; errors → ERROR + remove + `peer_selection` ← `Adversarial` + return).
///     Computes `min_ledger_slot = header.slot() - max_forecast`.
///     Conditionally refreshes cached `ledger_applied_slot` (via helper +
///     `eff.clock()`, rate-limited to 5s or initial, mod.rs:316-322; uses `VolatileTipEffect`).
///     Chooses `RollForwardMode`:
///     - If ledger slot < min → DEBUG "track_peers.defer_request_next" + `DeferTrailingRequestNext`.
///     - Else → `PipelineRequestNext`.
///       Then calls `execute_roll_forward`.
///
///   - `RollBackward(current, tip)`: INFO "roll backward" + *always* `handler` ← `RequestNext`.
///     Then `Store::load_tip` + `roll_backward` update (or on error: ERROR +
///     remove + `Adversarial`). (Tests: `test_roll_backward_*`.)
///
/// # Roll-Forward Execution & Modes
///
/// `execute_roll_forward` (called for both modes):
/// - `PipelineRequestNext`: sends `RequestNext` to handler *before* validation (pipelining).
/// - `DeferTrailingRequestNext`: *skips* the early send.
/// - Always: creates `Ledger`/`Store`, calls `validate_header` (era check via `era_history.slot_to_era_tag`,
///   parent/height/slot monotonicity vs. `per_peer.current`, plus `ledger.validate_header`; on any
///   error → ERROR + remove + `Adversarial` + return, mod.rs:133-182).
/// - On success: `roll_forward` (updates `current`/`highest`; `has_header`? no-op : `store_header`;
///   returns `Some(new_tip)` only on actual store) (mod.rs:184-202). On store success → send
///   `(tip, parent_point)` to `downstream`. On store error → remove + `Adversarial`.
/// - *Only* for `DeferTrailingRequestNext` (and only after success): `ensure_defer_req_next_stage` +
///   `Register { handler, min_ledger_slot }` to the child (mod.rs:267-270).
///
/// # The Defer Child Stage ("defer_req_next")
///
/// Lazily created exactly once (`ensure_defer_req_next_stage`):
/// - `eff.stage("track_peers/defer_req_next", defer_req_next::stage)` + `wire_up` + store the ref +
///   initial `Poll`.
/// - Protocol (`DeferReqNextMsg`):
///   - `Register { handler, min_ledger_slot }`: appends to `pending` vec (no immediate dispatch).
///   - `Poll`: `dispatch_ready` (queries `ledger_applied_slot` via shared helper, sends
///     `InitiatorMessage::RequestNext` to every handler where current ledger >= min_slot, retains
///     others) then `eff.schedule_after(Poll, Duration::from_millis(poll_interval_ms.max(1)))`.
///     Self-perpetuating polling loop once started.
/// - State: `DeferReqNext { poll_interval_ms, pending: Vec<(StageRef, Slot)> }`.
///   Created with the configured poll ms (default 200 in tests).
/// - Used exclusively to throttle `RequestNext` until the ledger has applied far enough
///   for the configured slot forecast. The child is never terminated. (Tests: `test_roll_forward_defers_*`
///   using `setup_with_ledger_tip` + max_forecast=0 + `tm_add_stage`/`tm_wire_stage_state`/
///   `assert_trace_does_not_contain` for immediate `RequestNext`.)
///
/// # External Effects, Scheduling, and Other Behaviours
/// - **Ledger**: `volatile_tip` (for applied height, via helper) + `validate_header` (with current span context).
/// - **Store**: `load_tip`, `has_header`, `store_header`.
/// - **Clock**: `eff.clock()` for 5s rate-limiting of height refreshes (mod.rs:317).
/// - **Scheduling**: Only inside the child (`schedule_after` for recurring `Poll`).
/// - **Sends** (via `eff.send`):
///   - To per-peer `handler`: `RequestNext` (pipelined or deferred) or `Done` (intersect stop).
///   - To `peer_selection`: only `Adversarial(peer)` on misbehaviour/errors.
///   - To `downstream`: `(Tip, Point)` (new tip + parent) on actual new-header store.
///   - To child: `Poll` (init), `Register`.
/// - No connection tracking beyond the `upstream` map + passed `handler` refs. No explicit
///   timeouts. No `terminate` on the stage itself.
/// - Logging levels: INFO (init/intersect/rollback), DEBUG (new/already-stored/defer decision),
///   TRACE (roll-forward entry), ERROR (failures), WARN (unknown intersect point).
///
/// # State Transitions
/// - `upstream` inserts on successful `IntersectFound` or test helper; mutates `current`/`highest`
///   on successful roll forward/backward; removes on any error or `IntersectNotFound`.
/// - Cached `ledger_applied_slot` / `ledger_last_checked_at` updated opportunistically.
/// - `defer_req_next` transitions from blackhole → wired ref exactly once (on first defer decision).
///
/// The stage is exercised exclusively via simulation harness in `test_setup.rs` (resource
/// injection for stores/validation, external effect overrides for ledger tip control,
/// `TraceEntry`/`TraceMatch` for effects and sends, `run_simulation` + `preload` of
/// `FromUpstream` msgs) and the tests in `tests.rs`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackPeers {
    era_history: EraHistory,
    upstream: BTreeMap<Peer, PerPeer>,
    peer_selection: StageRef<PeerSelectionMsg>,
    downstream: StageRef<(Tip, Point)>,
    max_forecast: SlotDelta,
    /// Lazily populated via [`Effects::stage`](amaru_pure_stage::Effects::stage) on first deferred `RequestNext`.
    defer_req_next: StageRef<DeferReqNextMsg>,
    defer_req_next_poll_ms: u64,
    ledger_applied_slot: Slot,
    ledger_last_checked_at: Instant,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PerPeer {
    current: Tip,
    highest: Tip,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TrackPeersMsg {
    FromUpstream(ChainSyncInitiatorMsg),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RollForwardMode {
    /// Send [`InitiatorMessage::RequestNext`](amaru_protocols::chainsync::InitiatorMessage::RequestNext) before validating (pipelined fetch).
    PipelineRequestNext,
    /// Skip the leading `RequestNext`; after a successful roll-forward, register a deferred `RequestNext`.
    DeferTrailingRequestNext { min_ledger_slot: Slot },
}

pub async fn stage(mut state: TrackPeers, msg: TrackPeersMsg, eff: Effects<TrackPeersMsg>) -> TrackPeers {
    match msg {
        TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg { peer, conn_id: _, handler, msg }) => {
            state.handle_from_upstream(peer, handler, msg, eff).await;
        }
    }
    state
}

impl TrackPeers {
    pub fn new(
        era_history: EraHistory,
        peer_selection: StageRef<PeerSelectionMsg>,
        downstream: StageRef<(Tip, Point)>,
        max_forecast: SlotDelta,
        defer_req_next_poll_ms: u64,
    ) -> Self {
        Self {
            era_history,
            upstream: BTreeMap::new(),
            peer_selection,
            downstream,
            max_forecast,
            defer_req_next: StageRef::blackhole(),
            defer_req_next_poll_ms,
            ledger_applied_slot: Slot::from(0),
            ledger_last_checked_at: Instant::at_offset(Duration::from_secs(0)),
        }
    }

    async fn ensure_defer_req_next_stage(&mut self, eff: &Effects<TrackPeersMsg>) {
        if !self.defer_req_next.is_blackhole() {
            return;
        }
        let defer_b = eff.stage("track_peers/defer_req_next", defer_req_next::stage).await;
        let state = defer_req_next::DeferReqNext::new(self.defer_req_next_poll_ms);
        let wired = eff.wire_up(defer_b, state).await;
        self.defer_req_next = wired;
        eff.send(&self.defer_req_next, DeferReqNextMsg::Poll).await;
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
        &self,
        peer: &Peer,
        variant: EraName,
        header: &BlockHeader,
        tip: Tip,
        ledger: &Ledger,
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

        // FIXME: check that slot time is within the permissible clock skew

        ledger
            .validate_header(header, Span::current().context())
            .await
            .map_err(|e| ConsensusError::InvalidHeader(header.point(), e))?;
        Ok(per_peer.current.point())
    }

    async fn roll_forward(
        &mut self,
        peer: &Peer,
        header: BlockHeader,
        tip: Tip,
        store: &Store,
    ) -> Result<Option<Tip>, ConsensusError> {
        let Some(per_peer) = self.upstream.get_mut(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        per_peer.current = header.tip();
        per_peer.highest = tip;
        if store.has_header(&header.hash()).await {
            Ok(None)
        } else {
            store.store_header(&header).await.map_err(|e| ConsensusError::StoreHeaderFailed(header.hash(), e))?;
            Ok(Some(per_peer.current))
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

    #[expect(clippy::too_many_arguments)]
    async fn execute_roll_forward(
        &mut self,
        peer: Peer,
        handler: StageRef<chainsync::InitiatorMessage>,
        variant: EraName,
        header: BlockHeader,
        tip: Tip,
        mode: RollForwardMode,
        eff: Effects<TrackPeersMsg>,
    ) {
        if matches!(mode, RollForwardMode::PipelineRequestNext) {
            eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
        }

        let ledger = Ledger::new(eff.clone());
        let store = Store::new(eff.clone());
        let result = self.validate_header(&peer, variant, &header, tip, &ledger).await;
        let parent = match result {
            Ok(parent) => parent,
            Err(error) => {
                tracing::error!(%error, %peer, "chain_sync.validate_header.failed");
                self.upstream.remove(&peer);
                eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer)).await;
                return;
            }
        };

        let current_point = header.point();
        match self.roll_forward(&peer, header, tip, &store).await {
            Ok(Some(tip)) => {
                tracing::debug!(%peer, tip = %tip.point(), "roll forward with new header");
                eff.send(&self.downstream, (tip, parent)).await;
            }
            Ok(None) => {
                tracing::debug!(%peer, tip = %current_point, "roll forward, header already stored");
            }
            Err(error) => {
                tracing::error!(%error, %peer, "chain_sync.store_header.failed");
                self.upstream.remove(&peer);
                eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer)).await;
                return;
            }
        };

        if let RollForwardMode::DeferTrailingRequestNext { min_ledger_slot } = mode {
            self.ensure_defer_req_next_stage(&eff).await;
            eff.send(&self.defer_req_next, DeferReqNextMsg::Register { handler, min_ledger_slot }).await;
        }
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
                tracing::trace!(%peer, variant = header_content.variant.as_str(), highest = %tip.point(), "roll forward");

                let variant = header_content.variant;
                let probe = decode_header(header_content, &peer);
                let header = match probe {
                    Ok(h) => h,
                    Err(error) => {
                        tracing::error!(%error, %peer, "chain_sync.decode_header.failed");
                        self.upstream.remove(&peer);
                        eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer)).await;
                        return;
                    }
                };

                let min_ledger_slot = Slot::new(header.slot().as_u64().saturating_sub(self.max_forecast.as_u64()));
                if min_ledger_slot > self.ledger_applied_slot
                    && let now = eff.clock().await
                    && (now.saturating_since(self.ledger_last_checked_at) > Duration::from_secs(5)
                        || self.ledger_applied_slot == Slot::from(0))
                {
                    self.ledger_last_checked_at = now;
                    self.ledger_applied_slot = ledger_applied_slot(&eff).await;
                }
                let mode = if self.ledger_applied_slot < min_ledger_slot {
                    tracing::debug!(
                        %peer,
                        header_slot = %header.slot(),
                        ledger_slot = %self.ledger_applied_slot,
                        limit = %min_ledger_slot,
                        "track_peers.defer_request_next",
                    );
                    RollForwardMode::DeferTrailingRequestNext { min_ledger_slot }
                } else {
                    RollForwardMode::PipelineRequestNext
                };

                self.execute_roll_forward(peer, handler, variant, header, tip, mode, eff).await;
            }
            RollBackward(current, tip) => {
                tracing::info!(%peer, %current, highest = %tip.point(), "roll backward");
                eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;

                let store = Store::new(eff.clone());
                if let Err(error) = self.roll_backward(&peer, current, tip, &store).await {
                    tracing::error!(%error, %peer, "chain_sync.roll_backward.failed");
                    self.upstream.remove(&peer);
                    eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer)).await;
                }
            }
        }
    }
}

pub fn decode_header(raw_header: HeaderContent, peer: &Peer) -> Result<BlockHeader, ConsensusError> {
    let _span = trace_span!(amaru_observability::amaru::consensus::chain_sync::DECODE_HEADER, peer = peer.to_string());
    let _guard = _span.enter();
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
