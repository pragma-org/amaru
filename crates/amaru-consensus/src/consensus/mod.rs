// Copyright 2024 PRAGMA
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

use crate::consensus::header_validation::assert_header;
use amaru_kernel::Point;
use amaru_ledger::ValidateBlockEvent;
use amaru_ouroboros::protocol::{peer, peer::*, PullEvent};
use amaru_ouroboros_traits::HasStakeDistribution;
use chain_selection::ChainSelector;
use gasket::framework::*;
use header::{point_hash, ConwayHeader, Header};
use miette::miette;
use pallas_codec::minicbor;
use pallas_crypto::hash::Hash;
use pallas_primitives::{babbage, conway::Epoch};
use pallas_traverse::ComputeHash;
use std::{collections::HashMap, sync::Arc};
use store::ChainStore;
use tokio::sync::Mutex;
use tracing::{instrument, trace, trace_span, Level, Span};

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateBlockEvent>;

const EVENT_TARGET: &str = "amaru::consensus";

pub mod chain_selection;
pub mod header;
pub mod header_validation;
pub mod nonce;
pub mod store;

#[derive(Stage)]
#[stage(name = "consensus.header", unit = "PullEvent", worker = "Worker")]
pub struct HeaderStage {
    peer_sessions: HashMap<Peer, PeerSession>,
    chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
    ledger: Box<dyn HasStakeDistribution>,
    store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>,
    epoch_to_nonce: HashMap<Epoch, Hash<32>>,

    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,

    #[metric]
    block_count: gasket::metrics::Counter,

    #[metric]
    rollback_count: gasket::metrics::Counter,

    #[metric]
    validation_tip: gasket::metrics::Gauge,
}

impl HeaderStage {
    pub fn new(
        peer_sessions: Vec<PeerSession>,
        ledger: Box<dyn HasStakeDistribution>,
        store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>,
        chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
        epoch_to_nonce: HashMap<Epoch, Hash<32>>,
    ) -> Self {
        let peer_sessions = peer_sessions
            .into_iter()
            .map(|p| (p.peer.clone(), p))
            .collect::<HashMap<_, _>>();
        Self {
            peer_sessions,
            chain_selector,
            ledger,
            store,
            epoch_to_nonce,
            upstream: Default::default(),
            downstream: Default::default(),
            block_count: Default::default(),
            rollback_count: Default::default(),
            validation_tip: Default::default(),
        }
    }

    fn track_validation_tip(&self, tip: &Point) {
        self.validation_tip.set(tip.slot_or_default() as i64);
    }

    async fn forward_block(
        &mut self,
        peer: &Peer,
        header: &dyn Header,
        parent_span: &Span,
    ) -> Result<(), WorkerError> {
        let point = header.point();
        let block = {
            // FIXME: should not crash if the peer is not found
            let peer_session = self
                .peer_sessions
                .get(peer)
                .expect("Unknown peer, bailing out");
            let mut session = peer_session.peer_client.lock().await;
            let client = (*session).blockfetch();
            let new_point: pallas_network::miniprotocols::Point = match point.clone() {
                Point::Origin => pallas_network::miniprotocols::Point::Origin,
                Point::Specific(slot, hash) => {
                    pallas_network::miniprotocols::Point::Specific(slot, hash)
                }
            };
            client.fetch_single(new_point).await.or_restart()?
        };

        self.downstream
            .send(ValidateBlockEvent::Validated(point, block, parent_span.clone()).into())
            .await
            .or_panic()
    }

    #[instrument(
        level = Level::INFO,
        skip_all,
        fields(
            peer = peer.name,
            rollback_point.slot = rollback_point.slot_or_default(),
            fork.length = fork.len(),
        ),
    )]
    async fn switch_to_fork(
        &mut self,
        peer: &Peer,
        rollback_point: &Point,
        fork: Vec<ConwayHeader>,
        parent_span: &Span,
    ) -> Result<(), WorkerError> {
        self.downstream
            .send(ValidateBlockEvent::Rollback(rollback_point.clone()).into())
            .await
            .or_panic()?;

        for header in fork {
            self.forward_block(peer, &header, parent_span).await?;
        }

        Ok(())
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        fields(
            peer = peer.name,
            point.slot = &point.slot_or_default(),
            point.hash = %point_hash(point),
        ),
    )]
    async fn handle_roll_forward(
        &mut self,
        peer: &Peer,
        point: &Point,
        raw_header: &[u8],
        parent_span: &Span,
    ) -> Result<(), WorkerError> {
        let span = trace_span!(
          target: EVENT_TARGET,
          parent: parent_span,
          "handle_roll_forward",
          slot = ?point.slot_or_default(),
          hash = point_hash(point).to_string())
        .entered();

        let header: babbage::MintedHeader<'_> = minicbor::decode(raw_header)
            .map_err(|e| miette!(e))
            .or_panic()?;

        // FIXME: move into chain_selector
        assert_header(&header, &self.epoch_to_nonce, self.ledger.as_ref())?;

        let header: ConwayHeader = ConwayHeader::from(header);

        // first make sure we store the header
        self.store
            .lock()
            .await
            .store_header(&header.compute_hash(), &header)
            .map_err(|e| miette!(e))
            .or_panic()?;

        let result = self
            .chain_selector
            .lock()
            .await
            .select_roll_forward(peer, header.clone());

        match result {
            chain_selection::ChainSelection::NewTip(hdr) => {
                trace!(target: EVENT_TARGET, hash = %hdr.hash(), "new_tip");
                self.forward_block(peer, &hdr, parent_span).await?;

                self.block_count.inc(1);
                self.track_validation_tip(point);
            }
            chain_selection::ChainSelection::RollbackTo(_) => {
                panic!("RollbackTo should never happen on a RollForward")
            }
            chain_selection::ChainSelection::SwitchToFork {
                peer,
                rollback_point,
                tip: _,
                fork,
            } => {
                self.switch_to_fork(&peer, &rollback_point, fork, parent_span)
                    .await?;
            }
            chain_selection::ChainSelection::NoChange => {
                trace!(target: EVENT_TARGET, hash = %header.hash(), "no_change");
            }
        }

        span.exit();

        Ok(())
    }

    #[instrument(level = Level::TRACE, skip(self, parent_span))]
    async fn handle_roll_back(
        &mut self,
        peer: &Peer,
        rollback: &Point,
        parent_span: &Span,
    ) -> Result<(), WorkerError> {
        let result = self
            .chain_selector
            .lock()
            .await
            .select_rollback(peer, point_hash(rollback));

        match result {
            chain_selection::ChainSelection::NewTip(_) => {
                panic!("cannot have a new tip on a rollback")
            }
            chain_selection::ChainSelection::RollbackTo(hash) => {
                trace!(target: EVENT_TARGET, %hash, "rollback");
                self.downstream
                    .send(ValidateBlockEvent::Rollback(rollback.clone()).into())
                    .await
                    .or_panic()?;
                self.rollback_count.inc(1);
                self.track_validation_tip(rollback);
            }
            chain_selection::ChainSelection::NoChange => {
                trace!(target: EVENT_TARGET, hash = %point_hash(rollback), "no_change");
            }
            chain_selection::ChainSelection::SwitchToFork {
                peer,
                rollback_point,
                fork,
                tip: _,
            } => {
                self.switch_to_fork(&peer, &rollback_point, fork, parent_span)
                    .await?
            }
        }

        Ok(())
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<HeaderStage> for Worker {
    async fn bootstrap(_stage: &HeaderStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut HeaderStage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &PullEvent,
        stage: &mut HeaderStage,
    ) -> Result<(), WorkerError> {
        match unit {
            PullEvent::RollForward(peer, point, raw_header, span) => {
                stage
                    .handle_roll_forward(peer, point, raw_header, span)
                    .await
            }
            PullEvent::Rollback(peer, rollback, span) => {
                stage.handle_roll_back(peer, rollback, span).await
            }
        }
    }
}
