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
    BlockHeader, BlockHeight, Epoch, EraHistory, EraName, IsHeader, ORIGIN_HASH, Peer, Point, Tip,
    from_cbor_no_leftovers,
};
use amaru_observability::trace_span;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    manager::ManagerMessage,
    store_effects::Store,
};
pub use defer_req_next::DeferReqNextMsg;
use pure_stage::{Effects, Instant, StageRef};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    effects::{Ledger, LedgerOps},
    errors::{ConsensusError, InvalidHeaderParentData, InvalidHeaderPoint},
};

/// Block height of the furthest ledger-applied state: volatile tip if present, otherwise stable tip.
pub(super) async fn ledger_applied_block_height<T: pure_stage::SendData + Sync>(eff: &Effects<T>) -> BlockHeight {
    let ledger = Ledger::new(eff.clone());
    let tip = match ledger.volatile_tip().await {
        Some(tip) => tip,
        None => ledger.tip().await,
    };
    tip.block_height()
}

/// This is the state of the [`stage`] that tracks peers from whom we are receiving headers.
///
/// It maintains the currently communicated tip as well as the highest advertised tip for each peer.
/// With this information, it validates incoming headers for protocol conformance and ensures that
/// they are stored in the chain store. When a new header is stored, its [`Tip`] is sent to the
/// `downstream` stage. The `manager` stage is used to remove peers that have violated the protocol.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackPeers {
    era_history: EraHistory,
    upstream: BTreeMap<Peer, PerPeer>,
    manager: StageRef<ManagerMessage>,
    downstream: StageRef<(Tip, Point)>,
    consensus_security_parameter: u64,
    /// Lazily populated via [`Effects::stage`](pure_stage::Effects::stage) on first deferred `RequestNext`.
    defer_req_next: StageRef<DeferReqNextMsg>,
    defer_req_next_poll_ms: u64,
    /// Headers whose validation failed because the ledger had not yet computed the required
    /// stake distribution. Drained on every `BlockApplied` and re-validated against the
    /// now-advanced ledger state.
    pending_validations: Vec<PendingValidation>,
    ledger_applied_block_height: BlockHeight,
    ledger_last_checked_at: Instant,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PerPeer {
    current: Tip,
    highest: Tip,
}

/// A header whose validation failed transiently and must be retried once the ledger has
/// caught up far enough to compute the required stake distribution.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PendingValidation {
    peer: Peer,
    handler: StageRef<chainsync::InitiatorMessage>,
    variant: EraName,
    header: BlockHeader,
    tip: Tip,
    missing_epoch: Epoch,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TrackPeersMsg {
    FromUpstream(ChainSyncInitiatorMsg),
    /// Sent by `adopt_chain` after a block has been adopted. This is used to re-validate pending headers.
    BlockApplied(Tip),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RollForwardMode {
    /// Send [`InitiatorMessage::RequestNext`](amaru_protocols::chainsync::InitiatorMessage::RequestNext) before validating (pipelined fetch).
    PipelineRequestNext,
    /// Skip the leading `RequestNext`; after a successful roll-forward, register a deferred `RequestNext`.
    DeferTrailingRequestNext { min_ledger_height: BlockHeight },
}

pub async fn stage(mut state: TrackPeers, msg: TrackPeersMsg, eff: Effects<TrackPeersMsg>) -> TrackPeers {
    match msg {
        TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg { peer, conn_id: _, handler, msg }) => {
            state.handle_from_upstream(peer, handler, msg, eff).await;
        }
        TrackPeersMsg::BlockApplied(tip) => {
            // Only retry pending validations whose `missing_epoch` boundary has been crossed by
            // the applied tip. Entries still waiting are kept and reconsidered on the next
            // `BlockApplied`.
            match state.era_history.slot_to_epoch_unchecked_horizon(tip.slot()) {
                Ok(applied_epoch) => {
                    let (ready, still_pending) = std::mem::take(&mut state.pending_validations)
                        .into_iter()
                        .partition::<Vec<_>, _>(|p| applied_epoch > p.missing_epoch);
                    state.pending_validations = still_pending;
                    for pending in ready {
                        state.handle_retry_validation(pending, eff.clone()).await;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "track_peers.block_applied.slot_to_epoch_failed");
                }
            }
        }
    }
    state
}

impl TrackPeers {
    pub fn new(
        era_history: EraHistory,
        manager: StageRef<ManagerMessage>,
        downstream: StageRef<(Tip, Point)>,
        consensus_security_parameter: u64,
        defer_req_next_poll_ms: u64,
    ) -> Self {
        Self {
            era_history,
            upstream: BTreeMap::new(),
            manager,
            downstream,
            consensus_security_parameter,
            defer_req_next: StageRef::blackhole(),
            defer_req_next_poll_ms,
            pending_validations: Vec::new(),
            ledger_applied_block_height: BlockHeight::from(0),
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

    /// Retry a previously deferred header validation.
    async fn handle_retry_validation(&mut self, pending: PendingValidation, eff: Effects<TrackPeersMsg>) {
        let mode = self.compute_rollforward_mode(pending.header.block_height(), &eff).await;
        self.execute_roll_forward(
            pending.peer,
            pending.handler,
            pending.variant,
            pending.header,
            pending.tip,
            mode,
            eff,
        )
        .await;
    }

    /// Decide whether to pipeline the next `RequestNext` or defer it, based on how far the
    /// ledger applied tip is behind the header being processed. Refreshes the cached
    /// `ledger_applied_block_height` if it might be stale and the comparison is close.
    async fn compute_rollforward_mode(
        &mut self,
        header_block_height: BlockHeight,
        eff: &Effects<TrackPeersMsg>,
    ) -> RollForwardMode {
        let min_ledger_height = header_block_height - self.consensus_security_parameter;
        if min_ledger_height > self.ledger_applied_block_height {
            let now = eff.clock().await;
            if now.saturating_since(self.ledger_last_checked_at) > Duration::from_secs(5)
                || self.ledger_applied_block_height == BlockHeight::from(0)
            {
                self.ledger_last_checked_at = now;
                self.ledger_applied_block_height = ledger_applied_block_height(eff).await;
            }
        }
        if self.ledger_applied_block_height < min_ledger_height {
            RollForwardMode::DeferTrailingRequestNext { min_ledger_height }
        } else {
            RollForwardMode::PipelineRequestNext
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
        &self,
        peer: &Peer,
        variant: EraName,
        header: BlockHeader,
        tip: Tip,
        ledger: &Ledger,
    ) -> Result<(BlockHeader, Point), ConsensusError> {
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

        // TODO: check that slot time is within the permissible clock skew

        ledger
            .validate_header(&header, Span::current().context())
            .await
            .map_err(|e| ConsensusError::InvalidHeader(header.point(), e))?;
        Ok((header, per_peer.current.point()))
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
        let tip = header.tip();
        if store.has_header(&header.hash()).await {
            Ok(None)
        } else {
            store.store_header(&header).await.map_err(|e| ConsensusError::StoreHeaderFailed(header.hash(), e))?;
            Ok(Some(tip))
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
        let ledger = Ledger::new(eff.clone());
        let store = Store::new(eff.clone());
        // Validate before sending `RequestNext` so a transient validation failure does not
        // leave us with the next header in flight that we have no way to validate either.
        let header_for_retry = header.clone();
        let result = self.validate_header(&peer, variant, header, tip, &ledger).await;
        let (header, parent) = match result {
            Ok(header) => header,
            Err(error) => {
                if let Some(epoch) = missing_stake_distribution(&error) {
                    // Transient issue: the ledger has not yet computed the stake distribution we need
                    // to validate this header. Keep the peer and stash the header so it can be
                    // retried once the ledger catches up. `per_peer.current` is unchanged so the
                    // retry sees the same predecessor.
                    tracing::debug!(
                        %peer, %epoch,
                        "chain_sync.validate_header.pending_stake_distribution"
                    );
                    self.pending_validations.push(PendingValidation {
                        peer,
                        handler,
                        variant,
                        header: header_for_retry,
                        tip,
                        missing_epoch: epoch,
                    });
                    return;
                }
                tracing::error!(%error, %peer, "chain_sync.validate_header.failed");
                self.upstream.remove(&peer);
                eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
                return;
            }
        };

        // Ask for the next header, if the ledger is not too far off.
        match mode {
            RollForwardMode::PipelineRequestNext => {
                eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
            }
            RollForwardMode::DeferTrailingRequestNext { min_ledger_height } => {
                self.ensure_defer_req_next_stage(&eff).await;
                eff.send(
                    &self.defer_req_next,
                    DeferReqNextMsg::Register { handler: handler.clone(), min_ledger_height },
                )
                .await;
            }
        }

        let tip_point = header.point();
        let tip = self.roll_forward(&peer, header, tip, &store).await;
        let tip = match tip {
            Ok(tip) => tip,
            Err(error) => {
                tracing::error!(%error, %peer, "chain_sync.store_header.failed");
                self.upstream.remove(&peer);
                eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
                return;
            }
        };

        if let Some(tip) = tip {
            tracing::debug!(%peer, tip = %tip.point(), "roll forward with new header");
            eff.send(&self.downstream, (tip, parent)).await;
        } else {
            tracing::debug!(%peer, tip = %tip_point, "roll forward, header already stored");
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
                        eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
                        return;
                    }
                };

                let mode = self.compute_rollforward_mode(header.block_height(), &eff).await;
                if let RollForwardMode::DeferTrailingRequestNext { min_ledger_height } = mode {
                    tracing::debug!(
                        %peer,
                        header_height = %header.block_height(),
                        ledger_height = %self.ledger_applied_block_height,
                        limit = %min_ledger_height,
                        "track_peers.defer_request_next",
                    );
                }

                self.execute_roll_forward(peer, handler, variant, header, tip, mode, eff).await;
            }
            RollBackward(current, tip) => {
                tracing::info!(%peer, %current, highest = %tip.point(), "roll backward");
                eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;

                let store = Store::new(eff.clone());
                if let Err(error) = self.roll_backward(&peer, current, tip, &store).await {
                    tracing::error!(%error, %peer, "chain_sync.roll_backward.failed");
                    self.upstream.remove(&peer);
                    eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
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

/// Inspect a `ConsensusError` and, if it carries a transient "stake distribution not available" error,
/// return the epoch whose distribution is missing. Otherwise return `None`.
#[expect(clippy::wildcard_enum_match_arm)]
fn missing_stake_distribution(error: &ConsensusError) -> Option<Epoch> {
    match error {
        ConsensusError::InvalidHeader(_, e) => e.as_missing_stake_distribution(),
        _ => None,
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
