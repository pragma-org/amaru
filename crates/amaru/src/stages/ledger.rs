use amaru_kernel::{
    into_owned_output, protocol_parameters::ProtocolParameters, Block, EraHistory, Hasher, Point,
};
use amaru_ledger::{
    context::{self, DefaultValidationContext},
    rules::{
        self,
        block::{BlockValidation, InvalidBlockDetails},
        parse_block,
    },
    state::{self, BackwardError, VolatileState},
    store::{HistoricalStores, Store},
    BlockValidationResult, RawBlock, ValidateBlockEvent,
};
use anyhow::Context;
use gasket::framework::{AsWorkError, WorkSchedule, WorkerError};
use std::sync::Arc;
use tracing::{instrument, Level, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub type UpstreamPort = gasket::messaging::InputPort<ValidateBlockEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<BlockValidationResult>;

pub struct ValidateBlockStage<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
    pub state: state::State<S, HS>,
}

impl<S: Store + Send, HS: HistoricalStores + Send> gasket::framework::Stage
    for ValidateBlockStage<S, HS>
{
    type Unit = ValidateBlockEvent;
    type Worker = Worker;

    fn name(&self) -> &str {
        "ledger"
    }

    fn metrics(&self) -> gasket::metrics::Registry {
        gasket::metrics::Registry::default()
    }
}

impl<S: Store + Send, HS: HistoricalStores + Send> ValidateBlockStage<S, HS> {
    pub fn new(store: S, snapshots: HS, era_history: &EraHistory) -> (Self, Point) {
        let state = state::State::new(
            Arc::new(std::sync::Mutex::new(store)),
            snapshots,
            era_history,
        );

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

    fn create_validation_context(
        &self,
        block: &Block<'_>,
    ) -> anyhow::Result<DefaultValidationContext> {
        let mut ctx = context::DefaultPreparationContext::new();

        rules::prepare_block(&mut ctx, block);

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
            .filter_map(|(input, opt_output)| {
                opt_output.map(|output| (input, into_owned_output(output)))
            })
            .collect();

        Ok(context::DefaultValidationContext::new(inputs))
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_forward"
    )]
    pub fn roll_forward(
        &mut self,
        point: Point,
        raw_block: RawBlock,
    ) -> anyhow::Result<Option<InvalidBlockDetails>> {
        let block = parse_block(&raw_block[..]).context("Failed to parse block")?;

        let mut context = self.create_validation_context(&block)?;

        match rules::validate_block(&mut context, ProtocolParameters::default(), &block) {
            BlockValidation::Err(err) => return Err(err),
            BlockValidation::Invalid(err) => {
                return Ok(Some(err));
            }
            BlockValidation::Valid(()) => {
                let state: VolatileState = context.into();
                let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);
                self.state.forward(state.anchor(&point, issuer))?;
                Ok(None)
            }
        }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_backward",
    )]
    pub async fn rollback_to(&mut self, point: Point, span: Span) -> BlockValidationResult {
        match self.state.backward(&point) {
            Ok(_) => BlockValidationResult::RolledBackTo {
                rollback_point: point,
                span,
            },
            Err(BackwardError::UnknownRollbackPoint(_)) => {
                BlockValidationResult::BlockValidationFailed { point, span }
            }
        }
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl<S: Store + Send, HS: HistoricalStores + Send>
    gasket::framework::Worker<ValidateBlockStage<S, HS>> for Worker
{
    async fn bootstrap(_stage: &ValidateBlockStage<S, HS>) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut ValidateBlockStage<S, HS>,
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
        stage: &mut ValidateBlockStage<S, HS>,
    ) -> Result<(), WorkerError> {
        let result = match unit {
            ValidateBlockEvent::Validated { point, block, span } => stage
                .roll_forward(point.clone(), block.to_vec())
                .map(|res| match res {
                    None => BlockValidationResult::BlockValidated {
                        point: point.clone(),
                        span: restore_span(span),
                    },
                    Some(_err) => BlockValidationResult::BlockValidationFailed {
                        point: point.clone(),
                        span: restore_span(span),
                    },
                })
                .or_panic()?,
            ValidateBlockEvent::Rollback {
                rollback_point,
                span,
            } => {
                stage
                    .rollback_to(rollback_point.clone(), restore_span(span))
                    .await
            }
        };

        Ok(stage.downstream.send(result.into()).await.or_panic()?)
    }
}

fn restore_span(parent_span: &Span) -> Span {
    let span = Span::current();
    span.set_parent(parent_span.context());
    span
}
