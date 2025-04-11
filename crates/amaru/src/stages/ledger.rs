use amaru_kernel::{protocol_parameters::ProtocolParameters, EraHistory, Hasher, Point};
use amaru_ledger::{
    context,
    rules::{self, block::InvalidBlock, parse_block},
    state::{self, BackwardError},
    store::Store,
    BlockValidationResult, RawBlock, ValidateBlockEvent,
};
use anyhow::Context;
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

    pub fn roll_forward(&mut self, point: Point, raw_block: RawBlock) -> anyhow::Result<()> {
        let mut ctx = context::DefaultPreparationContext::new();

        let block = parse_block(&raw_block[..]).context("Failed to parse block")?;

        let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);

        rules::prepare_block(&mut ctx, &block);

        // TODO: Eventually move into a separate function, or integrate within the ledger instead
        // of the current .resolve_inputs; once the latter is no longer needed for the state
        // construction.
        let inputs = self
            .state
            .resolve_inputs(&Default::default(), ctx.utxo.into_iter())
            .context("Failed to resolve inputs")?
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
        )?;

        self.state.forward(state.anchor(&point, issuer))?;

        Ok(())
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_forward"
    )]
    pub fn roll_forward_wrapper(
        &mut self,
        point: Point,
        raw_block: RawBlock,
        span: Span,
    ) -> anyhow::Result<BlockValidationResult> {
        match self.roll_forward(point.clone(), raw_block) {
            Ok(_) => {
                // TODO Make sure `roll_forward` returns a structured object encapsulating validation errors
                // Err should be used for unexpected errors only and stop block processing

                Ok(BlockValidationResult::BlockValidated(point, span))
            }
            Err(err) => match err.downcast_ref::<InvalidBlock>() {
                Some(_err) => Ok(BlockValidationResult::BlockValidationFailed(point, span)),
                None => Err(err),
            },
        }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_backward",
    )]
    pub async fn rollback_to(&mut self, point: Point, span: Span) -> BlockValidationResult {
        match self.state.backward(&point) {
            Ok(_) => BlockValidationResult::RolledBackTo(point, span),
            Err(BackwardError::UnknownRollbackPoint(_)) => {
                BlockValidationResult::BlockValidationFailed(point, span)
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
                let span = Span::current();
                span.set_parent(parent_span.context());
                stage
                    .roll_forward_wrapper(point.clone(), raw_block.to_vec(), span)
                    .or_panic()?
            }

            ValidateBlockEvent::Rollback(point, parent_span) => {
                // Restore parent span
                let span = Span::current();
                span.set_parent(parent_span.context());
                stage.rollback_to(point.clone(), span).await
            }
        };

        Ok(stage.downstream.send(result.into()).await.or_panic()?)
    }
}
