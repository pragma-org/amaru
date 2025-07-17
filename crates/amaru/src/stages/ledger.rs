use crate::{schedule, send, stages::common::adopt_current_span};
use amaru_consensus::IsHeader;
use amaru_kernel::{
    block::{BlockValidationResult, ValidateBlockEvent},
    protocol_parameters::GlobalParameters,
    EraHistory, Hash, Hasher, MintedBlock, Point, RawBlock,
};
use amaru_ledger::{
    context::{self, DefaultValidationContext},
    rules::{
        self,
        block::{BlockValidation, InvalidBlockDetails},
        parse_block,
    },
    state::{self, BackwardError, VolatileState},
    store::{HistoricalStores, Store, StoreError},
};
use anyhow::Context;
use gasket::framework::{WorkSchedule, WorkerError};
use std::sync::{Arc, RwLock};
use tracing::{error, info, instrument, trace, Level, Span};

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
    is_catching_up: Arc<RwLock<bool>>,
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
    pub fn new(
        store: S,
        snapshots: HS,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
        is_catching_up: Arc<RwLock<bool>>,
    ) -> Result<(Self, Point), StoreError> {
        let state = state::State::new(store, snapshots, era_history, global_parameters)?;

        let tip = state.tip().into_owned();

        Ok((
            Self {
                upstream: Default::default(),
                downstream: Default::default(),
                state,
                is_catching_up,
            },
            tip,
        ))
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name="ledger.create_validation_context",
        fields(
            block_body_hash = %block.header.header_body.block_body_hash,
            block_number = block.header.header_body.block_number,
            block_body_size = block.header.header_body.block_body_size,
            total_inputs
        )
    )]
    fn create_validation_context(
        &self,
        block: &MintedBlock<'_>,
    ) -> anyhow::Result<DefaultValidationContext> {
        let mut ctx = context::DefaultPreparationContext::new();
        rules::prepare_block(&mut ctx, block);
        tracing::Span::current().record("total_inputs", ctx.utxo.len());

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

        Ok(context::DefaultValidationContext::new(inputs))
    }

    /// Returns:
    /// * `Ok(Ok(u64))` - if no error occurred and the block is valid. `u64` is the blockheight.
    /// * `Ok(Err(<InvalidBlockDetails>))` - if no error occurred but block is invalid.
    /// * `Err(_)` - if an error occurred.
    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "ledger.roll_forward",
    )]
    pub fn roll_forward(
        &mut self,
        point: Point,
        raw_block: RawBlock,
    ) -> anyhow::Result<Result<u64, InvalidBlockDetails>> {
        let block = parse_block(&raw_block[..]).context("Failed to parse block")?;
        let mut context = self.create_validation_context(&block)?;
        let protocol_version = block.header.header_body.protocol_version;

        let is_catching_up = self.is_catching_up.read().map(|b| *b).unwrap_or(true);
        if is_catching_up {
            trace!(point.slot = %point.slot_or_default(), point.hash = %Hash::<32>::from(&point), "chain.extended");
        } else {
            info!(tip.slot = %point.slot_or_default(), tip.hash = %Hash::<32>::from(&point), "chain.extended");
        }

        match rules::validate_block(&mut context, self.state.protocol_parameters(), &block) {
            BlockValidation::Err(err) => Err(err),
            BlockValidation::Invalid(slot, id, err) => {
                error!("Block {id} invalid at slot={slot}: {}", err);
                Ok(Err(err))
            }
            BlockValidation::Valid(()) => {
                let state: VolatileState = context.into();
                let block_height = &block.header.block_height();
                let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);
                self.state
                    .forward(protocol_version, state.anchor(&point, issuer))?;
                Ok(Ok(*block_height))
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
        schedule!(&mut stage.upstream)
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
        adopt_current_span(unit);
        let result = match unit {
            ValidateBlockEvent::Validated { point, block, span } => {
                let point = point.clone();
                let block = block.to_vec();

                match stage.roll_forward(point.clone(), block.clone()) {
                    Ok(Ok(block_height)) => BlockValidationResult::BlockValidated {
                        point,
                        block,
                        span: span.clone(),
                        block_height,
                    },
                    Ok(Err(_)) => BlockValidationResult::BlockValidationFailed {
                        point,
                        span: span.clone(),
                    },
                    Err(err) => {
                        error!(?err, "Failed to validate block");
                        BlockValidationResult::BlockValidationFailed {
                            point,
                            span: span.clone(),
                        }
                    }
                }
            }
            ValidateBlockEvent::Rollback {
                rollback_point,
                span,
            } => {
                stage
                    .rollback_to(rollback_point.clone(), span.clone())
                    .await
            }
        };

        send!(&mut stage.downstream, result)
    }
}
