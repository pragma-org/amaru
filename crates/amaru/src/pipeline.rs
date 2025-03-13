use std::sync::Arc;

use amaru_kernel::{protocol_parameters::ProtocolParameters, Hash, MintedBlock, Point};
use gasket::framework::{AsWorkError, WorkSchedule, WorkerError};
use tracing::{info_span, instrument, trace_span, Level, Span};

use amaru_ledger::{
    rules,
    state::{self, BackwardError},
    store::Store,
    BlockValidationResult, RawBlock, ValidateBlockEvent,
};

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
            },
            tip,
        )
    }

    #[instrument(level = Level::TRACE, skip_all, fields(point.slot = ?point.slot_or_default(), point.hash = %Hash::<32>::from(&point), header.height = block.header.header_body.block_number, header.slot = block.header.header_body.slot, header.hash = hex::encode(block_header_hash)))]
    fn forward(
        &mut self,
        point: Point,
        block: MintedBlock<'_>,
        block_header_hash: Hash<32>,
    ) -> BlockValidationResult {
        let current_span = Span::current();
        match self.state.forward(&point, block) {
            Ok(_) => BlockValidationResult::BlockValidated(point, current_span),
            Err(_) => BlockValidationResult::BlockForwardStorageFailed(point, current_span),
        }
    }

    #[allow(clippy::panic)]
    pub async fn roll_forward(
        &mut self,
        point: Point,
        raw_block: RawBlock,
    ) -> BlockValidationResult {
        let (block_header_hash, block) =
            rules::validate_block(&raw_block[..], ProtocolParameters::default())
                .unwrap_or_else(|_| panic!("Failed to valdiate block"));

        self.forward(point, block, block_header_hash)
    }

    #[instrument(level = Level::TRACE, skip_all, fields(point.slot = point.slot_or_default(), point.hash = %Hash::<32>::from(&point)))]
    pub async fn rollback_to(&mut self, point: Point) -> BlockValidationResult {
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
                // Restore `parent_span` as current span
                let span = info_span!(
                    target: EVENT_TARGET,
                    parent: parent_span,
                    "roll_forward").entered();

                let result = stage.roll_forward(point.clone(), raw_block.to_vec()).await;

                span.exit();

                result
            }

            ValidateBlockEvent::Rollback(point) => stage.rollback_to(point.clone()).await,
        };

        Ok(stage.downstream.send(result.into()).await.or_panic()?)
    }
}
