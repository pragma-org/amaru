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

use std::collections::BTreeMap;

use amaru_kernel::{
    BlockHeader, BlockHeight, EraHistory, EraName, IsHeader, ORIGIN_HASH, Peer, Point, Tip, from_cbor_no_leftovers,
};
use amaru_observability::trace_span;
use amaru_ouroboros::ReadOnlyChainStore;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    manager::ManagerMessage,
    store_effects::Store,
};
pub use defer_req_next::DeferReqNextMsg;
use pure_stage::{Effects, StageRef};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    effects::{ConsensusEffects, ConsensusOps},
    errors::{ConsensusError, InvalidHeaderParentData, InvalidHeaderPoint},
};

/// Block height of the furthest ledger-applied state: volatile tip if present, otherwise stable tip.
pub(super) fn ledger_applied_block_height(eff: &impl ConsensusOps) -> BlockHeight {
    let ledger = eff.ledger();
    ledger.volatile_tip().unwrap_or_else(|| ledger.tip()).block_height()
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
    DeferTrailingRequestNext { min_ledger_height: BlockHeight },
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
        header: BlockHeader,
        tip: Tip,
        eff: impl ConsensusOps,
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

        eff.ledger()
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
        eff: impl ConsensusOps,
    ) -> Result<Option<Tip>, ConsensusError> {
        let Some(per_peer) = self.upstream.get_mut(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        per_peer.current = header.tip();
        per_peer.highest = tip;
        let tip = header.tip();
        if eff.store().has_header(&header.hash()) {
            Ok(None)
        } else {
            eff.store().store_header(&header).map_err(|e| ConsensusError::StoreHeaderFailed(header.hash(), e))?;
            Ok(Some(tip))
        }
    }

    async fn roll_backward(
        &mut self,
        peer: &Peer,
        current: Point,
        tip: Tip,
        eff: impl ConsensusOps,
    ) -> Result<(), ConsensusError> {
        let store = eff.store();
        let Some(header) = store.load_header(&current.hash()) else {
            return Err(ConsensusError::UnknownPoint(current.hash()));
        };
        let Some(per_peer) = self.upstream.get_mut(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        per_peer.current = Tip::new(current, header.block_height());
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

        let header = self.validate_header(&peer, variant, header, tip, ConsensusEffects::new(eff.clone())).await;
        let (header, parent) = match header {
            Ok(header) => header,
            Err(error) => {
                tracing::error!(%error, %peer, "chain_sync.validate_header.failed");
                self.upstream.remove(&peer);
                eff.send(&self.manager, ManagerMessage::RemovePeer(peer)).await;
                return;
            }
        };

        let tip_point = header.point();
        let tip = self.roll_forward(&peer, header, tip, ConsensusEffects::new(eff.clone())).await;
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

        if let RollForwardMode::DeferTrailingRequestNext { min_ledger_height } = mode {
            self.ensure_defer_req_next_stage(&eff).await;
            eff.send(
                &self.defer_req_next,
                DeferReqNextMsg::Register { peer: peer.clone(), handler, min_ledger_height },
            )
            .await;
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
                let Some(header) = Store::new(eff.clone()).load_header(&current.hash()) else {
                    tracing::warn!(%peer, %current, tip = %tip.point(), reason = "peer sent unknown intersection point", "stopping chainsync");
                    eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                    return;
                };
                tracing::info!(%peer, %current, highest = %tip.point(), "intersect found");
                let current = Tip::new(current, header.block_height());
                self.upstream.insert(peer, PerPeer { current, highest: tip });
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

                let consensus = ConsensusEffects::new(eff.clone());
                let ledger_h = ledger_applied_block_height(&consensus);
                let limit = self.consensus_security_parameter;
                let mode = if header.block_height() > ledger_h + limit {
                    tracing::debug!(
                        %peer,
                        header_height = %header.block_height(),
                        ledger_height = %ledger_h,
                        limit,
                        "track_peers.defer_request_next",
                    );
                    RollForwardMode::DeferTrailingRequestNext { min_ledger_height: header.block_height() - limit }
                } else {
                    RollForwardMode::PipelineRequestNext
                };

                self.execute_roll_forward(peer, handler, variant, header, tip, mode, eff).await;
            }
            RollBackward(current, tip) => {
                tracing::info!(%peer, %current, highest = %tip.point(), "roll backward");
                if !self.defer_req_next.is_blackhole() {
                    eff.send(&self.defer_req_next, DeferReqNextMsg::Cancel { peer: peer.clone() }).await;
                }
                eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;

                if let Err(error) = self.roll_backward(&peer, current, tip, ConsensusEffects::new(eff.clone())).await {
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

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
