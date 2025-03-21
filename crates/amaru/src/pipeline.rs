use amaru_kernel::{protocol_parameters::ProtocolParameters, Point};
use amaru_ledger::{
    rules::{self, context},
    state::{self, BackwardError},
    store::Store,
    BlockValidationResult, RawBlock, ValidateBlockEvent,
};
use gasket::framework::{AsWorkError, WorkSchedule, WorkerError};
use std::sync::Arc;
use tracing::{instrument, Level, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub type UpstreamPort = gasket::messaging::InputPort<ValidateBlockEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<BlockValidationResult>;

pub struct Stage<S>
where
    S: Store + Send + Sync,
{
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
    pub state: state::State<S>,
}

impl<S: Store + Send + Sync> gasket::framework::Stage for Stage<S> {
    type Unit = ValidateBlockEvent;
    type Worker = Worker;

    fn name(&self) -> &str {
        "ledger"
    }

    fn metrics(&self) -> gasket::metrics::Registry {
        gasket::metrics::Registry::default()
    }
}

impl<S: Store + Send + Sync> Stage<S> {
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

    #[allow(clippy::panic)]
    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_forward"
    )]
    pub async fn roll_forward(
        &mut self,
        point: Point,
        raw_block: RawBlock,
    ) -> BlockValidationResult {
        let (_block_header_hash, block) = rules::validate_block(
            &mut context::fake::FakeBlockValidationContext::new(),
            &raw_block[..],
            ProtocolParameters::default(),
        )
        .unwrap_or_else(|e| panic!("Failed to validate block: {:?}", e));

        let current_span = Span::current();

        match self.state.forward(&point, block) {
            Ok(()) => BlockValidationResult::BlockValidated(point, current_span),
            Err(_) => BlockValidationResult::BlockForwardStorageFailed(point, current_span),
        }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_backward",
    )]
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
impl<S: Store + Send + Sync> gasket::framework::Worker<Stage<S>> for Worker {
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

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.ledger"
    )]
    async fn execute(
        &mut self,
        unit: &ValidateBlockEvent,
        stage: &mut Stage<S>,
    ) -> Result<(), WorkerError> {
        let result = match unit {
            ValidateBlockEvent::Validated(point, raw_block, parent_span) => {
                // Restore parent span
                Span::current().set_parent(parent_span.context());
                stage.roll_forward(point.clone(), raw_block.to_vec()).await
            }

            ValidateBlockEvent::Rollback(point) => stage.rollback_to(point.clone()).await,
        };

        Ok(stage.downstream.send(result.into()).await.or_panic()?)
    }
}
