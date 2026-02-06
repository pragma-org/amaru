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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackPeers {
    upstream: BTreeMap<Peer, PerPeer>,
    manager: StageRef<ManagerMessage>,
    downstream: StageRef<Tip>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TrackPeersMsg {
    FromUpstream(ChainSyncInitiatorMsg),
}

pub async fn stage(
    mut state: TrackPeers,
    msg: TrackPeersMsg,
    eff: Effects<TrackPeersMsg>,
) -> TrackPeers {
    use TrackPeersMsg::*;

    match msg {
        FromUpstream(ChainSyncInitiatorMsg {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{ResourceHeaderValidation, ValidateHeaderEffect};
    use amaru_kernel::{BlockHeight, HeaderHash, Point, Tip, make_header};
    use amaru_ouroboros::ConnectionId;
    use amaru_ouroboros_traits::{
        ChainStore, can_validate_blocks::mock::MockCanValidateHeaders,
        in_memory_consensus_store::InMemConsensusStore,
    };
    use amaru_protocols::store_effects::{
        HasHeaderEffect, LoadHeaderEffect, ResourceHeaderStore, StoreHeaderEffect,
    };
    use opentelemetry::Context;
    use pure_stage::{
        DeserializerGuards, Effect, SendData, StageGraph,
        simulation::{SimulationBuilder, SimulationRunning},
        trace_buffer::{TraceBuffer, TraceEntry},
    };
    use std::sync::Arc;
    use tokio::runtime::{Builder, Handle};

    fn build_store(headers: &[BlockHeader]) -> Arc<InMemConsensusStore<BlockHeader>> {
        let store = Arc::new(InMemConsensusStore::new());
        for header in headers {
            store.store_header(header).unwrap();
        }
        store
    }

    fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> BlockHeader {
        BlockHeader::from(make_header(block_number, slot, parent))
    }

    fn validate_header_effect(at_stage: &str, header: BlockHeader) -> TraceEntry {
        TraceEntry::suspend(Effect::external(
            at_stage,
            Box::new(ValidateHeaderEffect::new(&header, Context::new())),
        ))
    }

    fn load_header_effect(at_stage: &str, hash: HeaderHash) -> TraceEntry {
        TraceEntry::suspend(Effect::external(
            at_stage,
            Box::new(LoadHeaderEffect::new(hash)),
        ))
    }

    fn has_header_effect(at_stage: &str, hash: HeaderHash) -> TraceEntry {
        TraceEntry::suspend(Effect::external(
            at_stage,
            Box::new(HasHeaderEffect::new(hash)),
        ))
    }

    fn store_header_effect(at_stage: &str, header: BlockHeader) -> TraceEntry {
        TraceEntry::suspend(Effect::external(
            at_stage,
            Box::new(StoreHeaderEffect::new(header)),
        ))
    }

    fn send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl SendData) -> TraceEntry {
        TraceEntry::suspend(Effect::send(from, to, Box::new(msg)))
    }

    fn setup(
        rt: &Handle,
        state: TrackPeers,
        msg: TrackPeersMsg,
        store: Arc<InMemConsensusStore<BlockHeader>>,
    ) -> (SimulationRunning, DeserializerGuards) {
        let guards = vec![
            pure_stage::register_data_deserializer::<TrackPeers>().boxed(),
            pure_stage::register_data_deserializer::<TrackPeersMsg>().boxed(),
            pure_stage::register_data_deserializer::<chainsync::InitiatorMessage>().boxed(),
            pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
            pure_stage::register_data_deserializer::<Tip>().boxed(),
            pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
            pure_stage::register_effect_deserializer::<HasHeaderEffect>().boxed(),
            pure_stage::register_effect_deserializer::<StoreHeaderEffect>().boxed(),
            pure_stage::register_effect_deserializer::<ValidateHeaderEffect>().boxed(),
        ];
        let mut network =
            SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1000000));
        network
            .resources()
            .put::<ResourceHeaderStore>(store.clone());
        network
            .resources()
            .put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
        let tp = network.stage("tp", stage);
        let tp = network.wire_up(tp, state);
        network.preload(&tp, [msg]).unwrap();
        let mut running = network.run();
        running.run_until_blocked_incl_effects(rt);
        (running, guards)
    }

    #[track_caller]
    fn assert_trace(running: &SimulationRunning, expected: &[TraceEntry]) {
        let mut tb = running.trace_buffer().lock();
        let trace = tb
            .iter_entries()
            .filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e))
            .collect::<Vec<_>>();
        tb.clear();
        pretty_assertions::assert_eq!(trace, expected);
    }

    #[test]
    fn test_new_peer() {
        let state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: Peer::new("peer1"),
            conn_id: ConnectionId::initial(),
            handler: StageRef::named_for_tests("handler"),
            msg: chainsync::InitiatorResult::Initialize,
        });

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }

    #[test]
    fn test_initialize_existing_peer() {
        let peer = Peer::new("peer1");
        let mut state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        state.upstream.insert(
            peer.clone(),
            PerPeer {
                current: Point::Origin,
                highest: Point::Origin,
            },
        );
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer,
            conn_id: ConnectionId::initial(),
            handler: StageRef::named_for_tests("handler"),
            msg: chainsync::InitiatorResult::Initialize,
        });

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }

    #[test]
    fn test_intersect_found_missing_header_sends_done() {
        let state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        let handler = StageRef::named_for_tests("handler");
        let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]));
        let tip = Tip::new(current, BlockHeight::from(1));
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: Peer::new("peer1"),
            conn_id: ConnectionId::initial(),
            handler: handler.clone(),
            msg: chainsync::InitiatorResult::IntersectFound(current, tip),
        });

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                load_header_effect("tp-1", current.hash()),
                send("tp-1", &handler, chainsync::InitiatorMessage::Done),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }

    #[test]
    fn test_intersect_found_tracks_peer() {
        let state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        let handler = StageRef::named_for_tests("handler");
        let header = make_block_header(1, 1, None);
        let current = header.point();
        let tip = Tip::new(current, BlockHeight::from(2));
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: Peer::new("peer1"),
            conn_id: ConnectionId::initial(),
            handler,
            msg: chainsync::InitiatorResult::IntersectFound(current, tip),
        });

        let mut expected = state.clone();
        expected.upstream.insert(
            Peer::new("peer1"),
            PerPeer {
                current,
                highest: tip.point(),
            },
        );

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(
            rt.handle(),
            state.clone(),
            msg.clone(),
            build_store(&[header]),
        );
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state)),
                TraceEntry::input("tp-1", Box::new(msg)),
                load_header_effect("tp-1", current.hash()),
                TraceEntry::state("tp-1", Box::new(expected)),
            ],
        );
    }

    #[test]
    fn test_intersect_not_found_untracked_sends_done() {
        let state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        let handler = StageRef::named_for_tests("handler");
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: Peer::new("peer1"),
            conn_id: ConnectionId::initial(),
            handler: handler.clone(),
            msg: chainsync::InitiatorResult::IntersectNotFound(Tip::origin()),
        });

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                send("tp-1", &handler, chainsync::InitiatorMessage::Done),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }

    #[test]
    fn test_intersect_not_found_removes_peer() {
        let peer = Peer::new("peer1");
        let mut state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        let expected = state.clone();
        state.upstream.insert(
            peer.clone(),
            PerPeer {
                current: Point::Origin,
                highest: Point::Origin,
            },
        );
        let handler = StageRef::named_for_tests("handler");
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer,
            conn_id: ConnectionId::initial(),
            handler: handler.clone(),
            msg: chainsync::InitiatorResult::IntersectNotFound(Tip::origin()),
        });

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state)),
                TraceEntry::input("tp-1", Box::new(msg)),
                send("tp-1", &handler, chainsync::InitiatorMessage::Done),
                TraceEntry::state("tp-1", Box::new(expected)),
            ],
        );
    }

    #[test]
    fn test_roll_forward_unknown_peer_removes() {
        let state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        let handler = StageRef::named_for_tests("handler");
        let header = make_block_header(1, 1, None);
        let child = make_block_header(2, 2, Some(header.hash()));
        let peer = Peer::new("peer1");
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: peer.clone(),
            conn_id: ConnectionId::initial(),
            handler: handler.clone(),
            msg: chainsync::InitiatorResult::RollForward(
                HeaderContent::new(&header, EraName::Conway),
                child.tip(),
            ),
        });

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                send("tp-1", &handler, chainsync::InitiatorMessage::RequestNext),
                send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }

    #[test]
    fn test_roll_forward_known_peer_header_already_stored() {
        let peer = Peer::new("peer1");
        let handler = StageRef::named_for_tests("handler");
        let parent = make_block_header(1, 1, None);
        let header = make_block_header(2, 2, Some(parent.hash()));
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: peer.clone(),
            conn_id: ConnectionId::initial(),
            handler: handler.clone(),
            msg: chainsync::InitiatorResult::RollForward(
                HeaderContent::new(&header, EraName::Conway),
                header.tip(),
            ),
        });

        let mut state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        state.upstream.insert(
            peer.clone(),
            PerPeer {
                current: parent.point(),
                highest: parent.point(),
            },
        );

        let mut expected = state.clone();
        expected.upstream.insert(
            peer.clone(),
            PerPeer {
                current: header.point(),
                highest: header.point(),
            },
        );

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(
            rt.handle(),
            state.clone(),
            msg.clone(),
            build_store(&[header.clone()]),
        );
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state)),
                TraceEntry::input("tp-1", Box::new(msg)),
                send("tp-1", &handler, chainsync::InitiatorMessage::RequestNext),
                validate_header_effect("tp-1", header.clone()),
                has_header_effect("tp-1", header.hash()),
                TraceEntry::state("tp-1", Box::new(expected)),
            ],
        );
    }

    #[test]
    fn test_roll_forward_known_peer_new_header_forwards_tip() {
        let peer = Peer::new("peer1");
        let handler = StageRef::named_for_tests("handler");
        let parent = make_block_header(1, 1, None);
        let header = make_block_header(2, 2, Some(parent.hash()));
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: peer.clone(),
            conn_id: ConnectionId::initial(),
            handler: handler.clone(),
            msg: chainsync::InitiatorResult::RollForward(
                HeaderContent::new(&header, EraName::Conway),
                header.tip(),
            ),
        });

        let mut state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        state.upstream.insert(
            peer.clone(),
            PerPeer {
                current: parent.point(),
                highest: parent.point(),
            },
        );

        let mut expected = state.clone();
        expected.upstream.insert(
            peer.clone(),
            PerPeer {
                current: header.point(),
                highest: header.point(),
            },
        );

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state)),
                TraceEntry::input("tp-1", Box::new(msg)),
                send("tp-1", &handler, chainsync::InitiatorMessage::RequestNext),
                validate_header_effect("tp-1", header.clone()),
                has_header_effect("tp-1", header.hash()),
                store_header_effect("tp-1", header.clone()),
                send("tp-1", "downstream", header.tip()),
                TraceEntry::state("tp-1", Box::new(expected)),
            ],
        );
    }

    #[test]
    fn test_roll_backward_updates_peer() {
        let peer = Peer::new("peer1");
        let header = make_block_header(1, 1, None);
        let current = header.point();
        let tip = Tip::new(current, BlockHeight::from(1));
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: peer.clone(),
            conn_id: ConnectionId::initial(),
            handler: StageRef::named_for_tests("handler"),
            msg: chainsync::InitiatorResult::RollBackward(current, tip),
        });

        let mut state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        state.upstream.insert(
            peer.clone(),
            PerPeer {
                current: Point::Origin,
                highest: Point::Origin,
            },
        );

        let mut expected = state.clone();
        expected.upstream.insert(
            peer,
            PerPeer {
                current,
                highest: tip.point(),
            },
        );

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(
            rt.handle(),
            state.clone(),
            msg.clone(),
            build_store(&[header]),
        );
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state)),
                TraceEntry::input("tp-1", Box::new(msg)),
                has_header_effect("tp-1", current.hash()),
                TraceEntry::state("tp-1", Box::new(expected)),
            ],
        );
    }

    #[test]
    fn test_roll_backward_unknown_peer_removes() {
        let peer = Peer::new("peer1");
        let header = make_block_header(1, 1, None);
        let current = header.point();
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: peer.clone(),
            conn_id: ConnectionId::initial(),
            handler: StageRef::named_for_tests("handler"),
            msg: chainsync::InitiatorResult::RollBackward(current, Tip::origin()),
        });

        let state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(
            rt.handle(),
            state.clone(),
            msg.clone(),
            build_store(&[header]),
        );
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                has_header_effect("tp-1", current.hash()),
                send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }

    #[test]
    fn test_roll_backward_unknown_point_removes() {
        let peer = Peer::new("peer1");
        let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]));
        let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: peer.clone(),
            conn_id: ConnectionId::initial(),
            handler: StageRef::named_for_tests("handler"),
            msg: chainsync::InitiatorResult::RollBackward(current, Tip::origin()),
        });

        let mut state = TrackPeers::new(
            StageRef::named_for_tests("manager"),
            StageRef::named_for_tests("downstream"),
        );
        state.upstream.insert(
            peer.clone(),
            PerPeer {
                current: Point::Origin,
                highest: Point::Origin,
            },
        );

        let rt = Builder::new_current_thread().build().unwrap();
        let (running, _guards) = setup(rt.handle(), state.clone(), msg.clone(), build_store(&[]));
        assert_trace(
            &running,
            &[
                TraceEntry::state("tp-1", Box::new(state.clone())),
                TraceEntry::input("tp-1", Box::new(msg)),
                has_header_effect("tp-1", current.hash()),
                send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
                TraceEntry::state("tp-1", Box::new(state)),
            ],
        );
    }
}
