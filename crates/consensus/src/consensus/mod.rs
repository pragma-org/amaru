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
use amaru_ledger::{RawBlock, ValidateHeaderEvent};
use amaru_ouroboros::protocol::{peer::*, Point, PullEvent};
use amaru_ouroboros::{ledger::LedgerState, protocol::peer};
use chain_selection::ChainSelector;
use gasket::framework::*;
use header::{point_hash, ConwayHeader, Header};
use miette::miette;
use pallas_codec::minicbor;
use pallas_crypto::hash::Hash;
use pallas_primitives::conway::Epoch;
use pallas_traverse::ComputeHash;
use std::{collections::HashMap, sync::Arc};
use store::ChainStore;
use tokio::sync::Mutex;
use tracing::instrument;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

pub mod chain_selection;
pub mod header;
pub mod header_validation;
pub mod nonce;
pub mod store;

#[derive(Stage)]
#[stage(name = "consensus", unit = "PullEvent", worker = "Worker")]
pub struct Stage {
    peer_session: PeerSession,
    chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
    ledger: Arc<Mutex<dyn LedgerState>>,
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

impl Stage {
    pub fn new(
        peer_session: PeerSession,
        ledger: Arc<Mutex<dyn LedgerState>>,
        store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>,
        chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
        epoch_to_nonce: HashMap<Epoch, Hash<32>>,
    ) -> Self {
        Self {
            peer_session,
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

    async fn forward_block(&mut self, header: &dyn Header) -> Result<(), WorkerError> {
        let point = header.point();
        let block = {
            let mut peer_session = self.peer_session.lock().await;
            let client = (*peer_session).blockfetch();
            client.fetch_single(point.clone()).await.or_restart()?
        };

        self.downstream
            .send(ValidateHeaderEvent::Validated(point, block).into())
            .await
            .or_panic()
    }

    async fn switch_to_fork(
        &mut self,
        rollback_point: &Point,
        fork: Vec<ConwayHeader>,
    ) -> Result<(), WorkerError> {
        self.downstream
            .send(ValidateHeaderEvent::Rollback(rollback_point.clone()).into())
            .await
            .or_panic()?;

        for header in fork {
            self.forward_block(&header).await?;
        }

        Ok(())
    }

    #[instrument(skip(self, raw_header))]
    async fn handle_roll_forward(
        &mut self,
        peer: &Peer,
        point: &Point,
        raw_header: &RawBlock,
    ) -> Result<(), WorkerError> {
        let header: ConwayHeader = minicbor::decode(raw_header)
            .map_err(|e| miette!(e))
            .or_panic()?;

        // first make sure we store the header
        self.store
            .lock()
            .await
            .put(&header.compute_hash(), &header)
            .map_err(|e| miette!(e))
            .or_panic()?;

        let ledger = self.ledger.lock().await;

        // FIXME: move into chain_selector
        assert_header(&header, raw_header, &self.epoch_to_nonce, &*ledger)?;

        let result = self
            .chain_selector
            .lock()
            .await
            .roll_forward(peer, header.clone());

        // Make sure the Mutex is released as soon as possible
        drop(ledger);

        match result {
            chain_selection::ChainSelection::NewTip(hdr) => {
                self.forward_block(&hdr).await?;

                self.block_count.inc(1);
                self.track_validation_tip(point);
            }
            chain_selection::ChainSelection::RollbackTo(_) => {
                panic!("RollbackTo should never happen on a RollForward")
            }
            chain_selection::ChainSelection::SwitchToFork(rollback_point, fork) => {
                self.switch_to_fork(&rollback_point, fork).await?;
            }
            chain_selection::ChainSelection::NoChange => (),
        }
        Ok(())
    }

    #[instrument(skip(self))]
    async fn handle_roll_back(&mut self, peer: &Peer, rollback: &Point) -> Result<(), WorkerError> {
        let result = self
            .chain_selector
            .lock()
            .await
            .rollback(peer, point_hash(rollback));

        match result {
            chain_selection::ChainSelection::NewTip(_) => {
                panic!("cannot have a new tip on a rollback")
            }
            chain_selection::ChainSelection::RollbackTo(_) => {
                self.downstream
                    .send(ValidateHeaderEvent::Rollback(rollback.clone()).into())
                    .await
                    .or_panic()?;
                self.rollback_count.inc(1);
                self.track_validation_tip(rollback);
            }
            chain_selection::ChainSelection::NoChange => (),
            chain_selection::ChainSelection::SwitchToFork(rollback_point, fork) => {
                self.switch_to_fork(&rollback_point, fork).await?
            }
        }

        Ok(())
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut Stage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(&mut self, unit: &PullEvent, stage: &mut Stage) -> Result<(), WorkerError> {
        match unit {
            PullEvent::RollForward(peer, point, raw_header) => {
                stage.handle_roll_forward(peer, point, raw_header).await
            }
            PullEvent::Rollback(peer, rollback) => stage.handle_roll_back(peer, rollback).await,
        }
    }
}
