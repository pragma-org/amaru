use std::sync::Arc;

use amaru_kernel::{protocol_parameters::ProtocolParameters, Point};
use gasket::framework::{AsWorkError, WorkSchedule, WorkerError};
use tracing::{trace_span, Span};

use amaru_ledger::{
    rules,
    state::{self, BackwardError},
    store::Store,
    BlockValidationResult, RawBlock, ValidateBlockEvent,
};

use amaru_mempool::{Mempool, SimpleMempool};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateBlockEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<BlockValidationResult>;

const EVENT_TARGET: &str = "amaru::ledger";

pub struct Stage<S>
where
    S: Store,
{
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
    pub state: state::State<S>,
    pub mempool: SimpleMempool,
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
                state,
                mempool: SimpleMempool::new(),
            },
            tip,
        )
    }

    #[allow(clippy::panic)]
    pub async fn roll_forward(
        &mut self,
        point: Point,
        raw_block: RawBlock,
        parent: &Span,
    ) -> BlockValidationResult {
        // TODO: use instrument macro
        let span_forward = trace_span!(
            target: EVENT_TARGET,
            parent: parent,
            "forward",
            header.height = tracing::field::Empty,
            header.slot = tracing::field::Empty,
            header.hash = tracing::field::Empty,
            stable.epoch = tracing::field::Empty,
            tip.epoch = tracing::field::Empty,
            tip.relative_slot = tracing::field::Empty,
        )
        .entered();

        let (block_header_hash, block) =
            rules::validate_block(&raw_block[..], ProtocolParameters::default())
                .unwrap_or_else(|_| panic!("Failed to valdiate block"));

        span_forward.record("header.height", block.header.header_body.block_number);
        span_forward.record("header.slot", block.header.header_body.slot);
        span_forward.record("header.hash", hex::encode(block_header_hash));

        let bodies = match &block.transaction_bodies {
            amaru_kernel::alonzo::MaybeIndefArray::Def(bodies) => {
                bodies
            }
            amaru_kernel::alonzo::MaybeIndefArray::Indef(bodies) => {
                bodies
            }
        };

        for body in bodies {
            let mut hash_set = std::collections::HashSet::new();
            for input in &body.inputs {
                hash_set.insert(input.clone());
            }
            self.mempool.invalidate_utxos(hash_set)
        }

        let result = match self.state.forward(&span_forward, &point, block) {
            Ok(_) => BlockValidationResult::BlockValidated(point, parent.clone()),
            Err(_) => BlockValidationResult::BlockForwardStorageFailed(point, parent.clone()),
        };

        span_forward.exit();
        result
    }

    pub async fn rollback_to(&mut self, point: Point) -> BlockValidationResult {
        let span_backward = trace_span!(
            target: EVENT_TARGET,
            "backward",
            point.slot = point.slot_or_default(),
            point.hash = tracing::field::Empty,
        );

        if let Point::Specific(_, header_hash) = &point {
            span_backward.record("point.hash", hex::encode(header_hash));
        }

        match self.state.backward(&point) {
            Ok(_) => BlockValidationResult::RolledBackTo(point),
            Err(BackwardError::UnknownRollbackPoint(_)) => {
                BlockValidationResult::InvalidRollbackPoint(point)
            }
        }
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
            ValidateBlockEvent::Validated(point, raw_block, parent_span) => {
                stage
                    .roll_forward(point.clone(), raw_block.to_vec(), parent_span)
                    .await
            }

            ValidateBlockEvent::Rollback(point) => stage.rollback_to(point.clone()).await,
        };

        Ok(stage.downstream.send(result.into()).await.or_panic()?)
    }
}
