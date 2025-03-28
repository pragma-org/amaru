use amaru_kernel::{network::EraHistory, protocol_parameters::ProtocolParameters, Hasher, Point};
use amaru_ledger::{
    context,
    rules::{self, parse_block},
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
    S: Store + Send,
{
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
    pub state: state::State<S>,
}

impl<S: Store + Send> gasket::framework::Stage for Stage<S> {
    type Unit = ValidateBlockEvent;
    type Worker = Worker;

    fn name(&self) -> &str {
        "ledger"
    }

    fn metrics(&self) -> gasket::metrics::Registry {
        gasket::metrics::Registry::default()
    }
}

impl<S: Store + Send> Stage<S> {
    pub fn new(store: S, era_history: &EraHistory) -> (Self, Point) {
        let state = state::State::new(Arc::new(std::sync::Mutex::new(store)), era_history);

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
        let mut ctx = context::DefaultPreparationContext::new();

        let block = parse_block(&raw_block[..])
            .unwrap_or_else(|e| panic!("Failed to parse block: {:?}", e));

        let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);

        rules::prepare_block(&mut ctx, &block);

        // TODO: Eventually move into a separate function, or integrate within the ledger instead
        // of the current .resolve_inputs; once the latter is no longer needed for the state
        // construction.
        let inputs = self
            .state
            .resolve_inputs(&Default::default(), ctx.utxo.into_iter())
            .unwrap_or_else(|e| panic!("Failed to resolve inputs: {e:?}"))
            .into_iter()
            // NOTE:
            // It isn't okay to just fail early here because we may be missing UTxO even on valid
            // transactions! Indeed, since we only have access to the _current_ volatile DB and the
            // immutable DB. That means, we can't be aware of UTxO created and used within the block.
            //
            // Those will however be produced during the validation, and be tracked by the
            // validation context.
            //
            // Hence, we *must* defer errors here until the moment we do expect the UTxO to be
            // present.
            .filter_map(|(input, opt_output)| opt_output.map(|output| (input, output)))
            .collect();

        let state = rules::validate_block(
            context::DefaultValidationContext::new(inputs),
            ProtocolParameters::default(),
            block,
        )
        .unwrap_or_else(|e| panic!("Failed to validate block: {:?}", e))
        .anchor(&point, issuer);

        let current_span = Span::current();

        match self.state.forward(state) {
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
impl<S: Store + Send> gasket::framework::Worker<Stage<S>> for Worker {
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
