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

use crate::kernel::{Hash, Hasher, MintedBlock, Point};
use gasket::framework::*;
use pallas_codec::minicbor as cbor;
use state::BackwardError;
use std::sync::Arc;
use store::Store;
use tokio::sync::Mutex;
use tracing::{debug_span, instrument, warn, Level};

const EVENT_TARGET: &str = "amaru::ledger";

pub type RawBlock = Vec<u8>;

#[derive(Clone)]
pub enum ValidateBlockEvent {
    Validated(Point, RawBlock),
    Rollback(Point),
}

#[derive(Clone)]
pub enum BlockValidationResult {
    BlockValidated(Point),
    BlockForwardStorageFailed(Point),
    InvalidRollbackPoint(Point),
    RolledBackTo(Point),
}

pub type UpstreamPort = gasket::messaging::InputPort<ValidateBlockEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<BlockValidationResult>;

/// Iterators
///
/// A set of additional primitives around iterators. Not Amaru-specific so-to-speak.
pub mod iter;
pub mod kernel;
pub mod rewards;
pub mod state;
pub mod store;

pub struct Stage<S>
where
    S: Store,
{
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
    pub state: Arc<Mutex<state::State<S>>>,
}

impl<S: Store> gasket::framework::Stage for Stage<S> {
    type Unit = ValidateBlockEvent;
    type Worker = Worker;

    fn name(&self) -> &str {
        "ledger"
    }

    fn metrics(&self) -> gasket::metrics::Registry {
        gasket::metrics::Registry::default()
    }
}

impl<S: Store> Stage<S> {
    pub fn new(store: S) -> (Self, Point) {
        let state = state::State::new(Arc::new(std::sync::Mutex::new(store)));

        let tip = state.tip().into_owned();

        (
            Self {
                upstream: Default::default(),
                downstream: Default::default(),
                state: Arc::new(Mutex::new(state)),
            },
            tip,
        )
    }

    pub async fn validate_block(
        &mut self,
        point: Point,
        raw_block: RawBlock,
    ) -> BlockValidationResult {
        // TODO: use instrument macro
        let span_forward = debug_span!(
            target: EVENT_TARGET,
            "forward",
            header.height = tracing::field::Empty,
            header.slot = tracing::field::Empty,
            header.hash = tracing::field::Empty,
            stable.epoch = tracing::field::Empty,
            tip.epoch = tracing::field::Empty,
            tip.relative_slot = tracing::field::Empty,
        )
        .entered();

        let (block_header_hash, block) = parse_block(&raw_block[..]);

        span_forward.record("header.height", block.header.header_body.block_number);
        span_forward.record("header.slot", block.header.header_body.slot);
        span_forward.record("header.hash", hex::encode(block_header_hash));

        let mut state = self.state.lock().await;

        let result = match state.forward(&span_forward, &point, block) {
            Ok(_) => BlockValidationResult::BlockValidated(point),
            Err(_) => BlockValidationResult::BlockForwardStorageFailed(point),
        };

        span_forward.exit();
        result
    }

    pub async fn rollback_to(&mut self, point: Point) -> BlockValidationResult {
        let span_backward = debug_span!(
            target: EVENT_TARGET,
            "backward",
            point.slot = point.slot_or_default(),
            point.hash = tracing::field::Empty,
        )
        .entered();

        if let Point::Specific(_, header_hash) = &point {
            span_backward.record("point.hash", hex::encode(header_hash));
        }

        let mut state = self.state.lock().await;

        let result = match state.backward(&point) {
            Ok(_) => BlockValidationResult::RolledBackTo(point),
            Err(BackwardError::UnknownRollbackPoint(_)) => {
                BlockValidationResult::InvalidRollbackPoint(point)
            }
        };

        span_backward.exit();

        result
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl<S: Store> gasket::framework::Worker<Stage<S>> for Worker {
    async fn bootstrap(_stage: &Stage<S>) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut Stage<S>,
    ) -> Result<WorkSchedule<ValidateBlockEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;
        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &ValidateBlockEvent,
        stage: &mut Stage<S>,
    ) -> Result<(), WorkerError> {
        let result = match unit {
            ValidateBlockEvent::Validated(point, raw_block) => {
                stage
                    .validate_block(point.clone(), raw_block.to_vec())
                    .await
            }

            ValidateBlockEvent::Rollback(point) => stage.rollback_to(point.clone()).await,
        };

        Ok(stage.downstream.send(result.into()).await.or_panic()?)
    }
}

#[instrument(level = Level::DEBUG, skip(bytes), fields(block.size = bytes.len()))]
fn parse_block(bytes: &[u8]) -> (Hash<32>, MintedBlock<'_>) {
    let (_, block): (u16, MintedBlock<'_>) = cbor::decode(bytes)
        .unwrap_or_else(|_| panic!("failed to decode Conway block: {:?}", hex::encode(bytes)));

    (Hasher::<256>::hash(block.header.raw_cbor()), block)
}
