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

use std::collections::BTreeMap;

use amaru_kernel::{BlockHeader, EraHistory, EraName, IsHeader, ORIGIN_HASH, Peer, Point, Tip, from_cbor_no_leftovers};
use amaru_ouroboros::ReadOnlyChainStore;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    manager::ManagerMessage,
    store_effects::Store,
};
use pure_stage::{Effects, StageRef};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    effects::{ConsensusEffects, ConsensusOps},
    errors::{ConsensusError, InvalidHeaderParentData, InvalidHeaderPoint},
};

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
    downstream: StageRef<Tip>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PerPeer {
    current: Tip,
    highest: Tip,
}

impl TrackPeers {
    pub fn new(era_history: EraHistory, manager: StageRef<ManagerMessage>, downstream: StageRef<Tip>) -> Self {
        Self { era_history, upstream: BTreeMap::new(), manager, downstream }
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
        raw_header: HeaderContent,
        tip: Tip,
        eff: impl ConsensusOps,
    ) -> Result<BlockHeader, ConsensusError> {
        let variant = raw_header.variant;
        let header = decode_header(raw_header)?;
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
        let highest = tip.point();
        if header.point() < per_peer.current.point() || header.point() > highest {
            return Err(ConsensusError::InvalidHeaderPoint(Box::new(InvalidHeaderPoint {
                actual: header.point(),
                parent: per_peer.current.point(),
                highest,
            })));
        }
        eff.ledger()
            .validate_header(&header, Span::current().context())
            .await
            .map_err(|e| ConsensusError::InvalidHeader(header.point(), e))?;
        Ok(header)
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
}

#[tracing::instrument(
    level = tracing::Level::TRACE,
    skip_all,
    name = "chain_sync.decode_header",
)]
pub fn decode_header(raw_header: HeaderContent) -> Result<BlockHeader, ConsensusError> {
    // need to list all the variants supported by the current Amaru implementation
    if !matches!(raw_header.variant, EraName::Conway) {
        return Err(ConsensusError::InvalidHeaderVariant(raw_header.variant));
    }
    from_cbor_no_leftovers(&raw_header.cbor)
        .map_err(|reason| ConsensusError::CannotDecodeHeader { header: raw_header.cbor, reason: reason.to_string() })
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TrackPeersMsg {
    FromUpstream(ChainSyncInitiatorMsg),
}

pub async fn stage(mut state: TrackPeers, msg: TrackPeersMsg, eff: Effects<TrackPeersMsg>) -> TrackPeers {
    use TrackPeersMsg::*;

    match msg {
        FromUpstream(ChainSyncInitiatorMsg { peer, conn_id: _, handler, msg }) => {
            use amaru_protocols::chainsync::InitiatorResult::*;
            match msg {
                Initialize => {
                    tracing::info!(%peer,"initializing chainsync");
                }
                IntersectFound(current, tip) => {
                    let Some(header) = Store::new(eff.clone()).load_header(&current.hash()) else {
                        tracing::warn!(%peer, %current, %tip, reason = "peer sent unknown intersection point", "stopping chainsync");
                        eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                        return state;
                    };
                    tracing::info!(%peer, %current, highest = %tip.point(), "intersect found");
                    let current = Tip::new(current, header.block_height());
                    state.upstream.insert(peer, PerPeer { current, highest: tip });
                }
                IntersectNotFound(tip) => {
                    tracing::info!(%peer, highest = %tip.point(), reason = "intersect not found", "stopping chainsync");
                    eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                    state.upstream.remove(&peer);
                }
                RollForward(header_content, tip) => {
                    tracing::trace!(%peer, variant = header_content.variant.as_str(), highest = %tip.point(), "roll forward");
                    eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;

                    let header =
                        state.validate_header(&peer, header_content, tip, ConsensusEffects::new(eff.clone())).await;
                    let header = match header {
                        Ok(header) => header,
                        Err(error) => {
                            tracing::error!(%error, %peer, "chain_sync.validate_header.failed");
                            state.upstream.remove(&peer);
                            eff.send(&state.manager, ManagerMessage::RemovePeer(peer)).await;
                            return state;
                        }
                    };

                    let tip_point = tip.point();
                    let tip = state.roll_forward(&peer, header, tip, ConsensusEffects::new(eff.clone())).await;
                    let tip = match tip {
                        Ok(tip) => tip,
                        Err(error) => {
                            tracing::error!(%error, %peer, "chain_sync.store_header.failed");
                            state.upstream.remove(&peer);
                            eff.send(&state.manager, ManagerMessage::RemovePeer(peer)).await;
                            return state;
                        }
                    };

                    if let Some(tip) = tip {
                        tracing::debug!(%peer, tip = %tip.point(), "roll forward with new header");
                        eff.send(&state.downstream, tip).await;
                    } else {
                        tracing::debug!(%peer, tip = %tip_point, "roll forward, header already stored");
                    }
                }
                RollBackward(current, tip) => {
                    tracing::info!(%peer, %current, highest = %tip.point(), "roll backward");

                    if let Err(error) =
                        state.roll_backward(&peer, current, tip, ConsensusEffects::new(eff.clone())).await
                    {
                        tracing::error!(%error, %peer, "chain_sync.roll_backward.failed");
                        state.upstream.remove(&peer);
                        eff.send(&state.manager, ManagerMessage::RemovePeer(peer)).await;
                        return state;
                    }
                }
            }
        }
    }
    state
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
