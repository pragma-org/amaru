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

use crate::{consensus::header_validation::assert_header, sync::PullEvent};
use amaru_ledger::ValidateHeaderEvent;
use chain_selection::ChainSelector;
use gasket::framework::*;
use header::ConwayHeader;
use miette::miette;
use ouroboros::ledger::LedgerState;
use pallas_codec::minicbor;
use pallas_crypto::hash::Hash;
use pallas_network::miniprotocols::Point::*;
use pallas_primitives::conway::Epoch;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use store::{ChainStore, SimpleChainStore};
use tokio::sync::Mutex;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;
pub type Point = pallas_network::miniprotocols::Point;

pub mod chain_selection;
pub mod header;
pub mod header_validation;
pub mod nonce;
pub mod peer;
pub mod store;

pub use peer::*;

#[derive(Stage)]
#[stage(name = "header_validation", unit = "PullEvent", worker = "Worker")]
pub struct Stage {
    peer_session: PeerSession,
    tip: Point,
    chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
    ledger: Arc<Mutex<dyn LedgerState>>,
    epoch_to_nonce: HashMap<Epoch, Hash<32>>,
    store: SimpleChainStore,

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
        tip: Point,
        ledger: Arc<Mutex<dyn LedgerState>>,
        epoch_to_nonce: HashMap<Epoch, Hash<32>>,
    ) -> Self {
        let store = SimpleChainStore::new(PathBuf::from("."));
        let chain_selector = Arc::new(Mutex::new(ChainSelector::empty()));
        Self {
            peer_session,
            tip,
            chain_selector,
            ledger,
            epoch_to_nonce,
            store,
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
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        // initialise chain selector with the current tip
        let mut chain_selector = stage.chain_selector.lock().await;
        match &stage.tip {
            Origin => todo!(),
            Specific(_, header_hash) => {
                let slice = header_hash.as_slice();
                match stage.store.get(&Hash::from(slice)) {
                    None => todo!(),
                    Some(header) => chain_selector.set_tip(&header),
                }
            }
        };

        // initialise chain selector with peer(s)
        let peer = &stage.peer_session.peer;
        chain_selector.add_peer(peer);

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
            PullEvent::RollForward(point, raw_header) => {
                let header: ConwayHeader = minicbor::decode(raw_header)
                    .map_err(|e| miette!(e))
                    .or_panic()?;

                let ledger = stage.ledger.lock().await;

                assert_header(&header, raw_header, &stage.epoch_to_nonce, &*ledger)?;

                // Make sure the Mutex is released as soon as possible
                drop(ledger);

                let block = {
                    let mut peer_session = stage.peer_session.lock().await;
                    let client = (*peer_session).blockfetch();
                    client.fetch_single(point.clone()).await.or_restart()?
                };

                stage
                    .downstream
                    .send(ValidateHeaderEvent::Validated(point.clone(), block).into())
                    .await
                    .or_panic()?;

                stage.block_count.inc(1);
                stage.track_validation_tip(point);
            }
            PullEvent::Rollback(rollback) => {
                stage
                    .downstream
                    .send(ValidateHeaderEvent::Rollback(rollback.clone()).into())
                    .await
                    .or_panic()?;

                stage.rollback_count.inc(1);
                stage.track_validation_tip(rollback);
            }
        }

        Ok(())
    }
}
