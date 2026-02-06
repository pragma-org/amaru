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

use crate::{
    effects::{ConsensusEffects, ConsensusOps},
    errors::{ConsensusError, InvalidHeaderParentData},
};
use amaru_kernel::{
    BlockHeader, EraName, IsHeader, ORIGIN_HASH, Peer, Point, Tip, from_cbor_no_leftovers,
};
use amaru_ouroboros::ReadOnlyChainStore;
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent},
    manager::ManagerMessage,
    store_effects::Store,
};
use pure_stage::{Effects, StageRef};
use std::collections::BTreeMap;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackPeers {
    upstream: BTreeMap<Peer, PerPeer>,
    manager: StageRef<ManagerMessage>,
    downstream: StageRef<Tip>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct PerPeer {
    current: Point,
    highest: Point,
}

impl TrackPeers {
    pub fn new(manager: StageRef<ManagerMessage>, downstream: StageRef<Tip>) -> Self {
        Self {
            upstream: BTreeMap::new(),
            manager,
            downstream,
        }
    }

    async fn validate_header(
        &self,
        peer: &Peer,
        raw_header: HeaderContent,
        tip: Tip,
        eff: impl ConsensusOps,
    ) -> Result<BlockHeader, ConsensusError> {
        let header = decode_header(raw_header)?;
        let Some(per_peer) = self.upstream.get(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        if header.parent_hash().unwrap_or(ORIGIN_HASH) != per_peer.current.hash() {
            return Err(ConsensusError::InvalidHeaderParent(Box::new(
                InvalidHeaderParentData {
                    peer: peer.clone(),
                    forwarded: header.point(),
                    actual: header.parent_hash(),
                    expected: per_peer.current,
                },
            )));
        }
        let highest = per_peer.highest.max(tip.point());
        if header.point() < per_peer.current || header.point() > highest {
            return Err(ConsensusError::InvalidHeaderPoint {
                actual: header.point(),
                parent: per_peer.current,
                highest: per_peer.highest,
            });
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
        per_peer.current = header.point();
        per_peer.highest = tip.point();
        if eff.store().has_header(&header.hash()) {
            Ok(None)
        } else {
            eff.store()
                .store_header(&header)
                .map_err(|e| ConsensusError::StoreHeaderFailed(header.hash(), e))?;
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
        if !eff.store().has_header(&current.hash()) {
            return Err(ConsensusError::UnknownPoint(current.hash()));
        }
        let Some(per_peer) = self.upstream.get_mut(peer) else {
            return Err(ConsensusError::UnknownPeer(peer.clone()));
        };
        per_peer.current = current;
        per_peer.highest = tip.point();
        Ok(())
    }
}

#[tracing::instrument(
    level = tracing::Level::TRACE,
    skip_all,
    name = "chain_sync.decode_header",
)]
pub fn decode_header(raw_header: HeaderContent) -> Result<BlockHeader, ConsensusError> {
    if raw_header.variant != EraName::Conway {
        return Err(ConsensusError::InvalidHeaderVariant(raw_header.variant));
    }
    from_cbor_no_leftovers(&raw_header.cbor).map_err(|reason| ConsensusError::CannotDecodeHeader {
        header: raw_header.cbor,
        reason: reason.to_string(),
    })
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TrackPeersMsg {
    FromUpstrea(ChainSyncInitiatorMsg),
}

pub async fn stage(
    mut state: TrackPeers,
    msg: TrackPeersMsg,
    eff: Effects<TrackPeersMsg>,
) -> TrackPeers {
    use TrackPeersMsg::*;

    match msg {
        FromUpstrea(ChainSyncInitiatorMsg {
            peer,
            conn_id: _,
            handler,
            msg,
        }) => {
            use amaru_protocols::chainsync::InitiatorResult::*;
            match msg {
                Initialize => {
                    tracing::info!(%peer,"initializing chainsync");
                }
                IntersectFound(current, tip) => {
                    if Store::new(eff.clone())
                        .load_header(&current.hash())
                        .is_none()
                    {
                        tracing::warn!(%peer, %current, %tip, reason = "peer sent unknown intersection point", "stopping chainsync");
                        eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                        return state;
                    }
                    let highest = tip.point();
                    tracing::info!(%peer, %current, %highest, "intersect found");
                    state.upstream.insert(peer, PerPeer { current, highest });
                }
                IntersectNotFound(tip) => {
                    tracing::info!(%peer, highest = %tip.point(), reason = "intersect not found", "stopping chainsync");
                    eff.send(&handler, chainsync::InitiatorMessage::Done).await;
                    state.upstream.remove(&peer);
                }
                RollForward(header_content, tip) => {
                    let highest = tip.point();
                    tracing::trace!(%peer, variant = header_content.variant.as_str(), %highest, "roll forward");
                    eff.send(&handler, chainsync::InitiatorMessage::RequestNext)
                        .await;

                    let header = state
                        .validate_header(
                            &peer,
                            header_content,
                            tip,
                            ConsensusEffects::new(eff.clone()),
                        )
                        .await;
                    let header = match header {
                        Ok(header) => header,
                        Err(error) => {
                            tracing::error!(%error, %peer, "chain_sync.validate_header.failed");
                            eff.send(&state.manager, ManagerMessage::RemovePeer(peer))
                                .await;
                            return state;
                        }
                    };

                    let tip = state
                        .roll_forward(&peer, header, tip, ConsensusEffects::new(eff.clone()))
                        .await;
                    let tip = match tip {
                        Ok(tip) => tip,
                        Err(error) => {
                            tracing::error!(%error, %peer, "chain_sync.store_header.failed");
                            eff.send(&state.manager, ManagerMessage::RemovePeer(peer))
                                .await;
                            return state;
                        }
                    };

                    if let Some(tip) = tip {
                        eff.send(&state.downstream, tip).await;
                    }
                }
                RollBackward(current, tip) => {
                    let highest = tip.point();
                    tracing::info!(%peer, %current, %highest, "roll backward");

                    if let Err(error) = state
                        .roll_backward(&peer, current, tip, ConsensusEffects::new(eff.clone()))
                        .await
                    {
                        tracing::error!(%error, %peer, "chain_sync.roll_backward.failed");
                        eff.send(&state.manager, ManagerMessage::RemovePeer(peer))
                            .await;
                        return state;
                    }
                }
            }
        }
    }
    state
}
