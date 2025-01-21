use crate::{
    kernel::{Hash, Hasher, MintedBlock, Point},
    state::BackwardErr,
};
use gasket::framework::*;
use opentelemetry::metrics::Counter;
use pallas_codec::minicbor as cbor;
use std::sync::Arc;
use store::Store;
use tokio::sync::Mutex;
use tracing::{error, info_span, warn};

const EVENT_TARGET: &str = "amaru::ledger";

pub type RawBlock = Vec<u8>;

#[derive(Clone)]
pub enum ValidateHeaderEvent {
    Validated(Point, RawBlock),
    Rollback(Point),
}

pub type UpstreamPort = gasket::messaging::InputPort<ValidateHeaderEvent>;

/// Iterators
///
/// A set of additional primitives around iterators. Not Amaru-specific so-to-speak.
pub mod iter;
pub mod kernel;
pub mod rewards;
pub mod state;
pub mod store;

pub struct Stage<T>
where
    T: Store + Send + Sync,
{
    pub upstream: UpstreamPort,
    pub state: Arc<Mutex<state::State<T, T::Error>>>,
    pub counter: Counter<u64>,
}

impl<T: Store + Send + Sync> gasket::framework::Stage for Stage<T> {
    type Unit = ValidateHeaderEvent;
    type Worker = Worker;

    fn name(&self) -> &str {
        "ledger"
    }

    fn metrics(&self) -> gasket::metrics::Registry {
        gasket::metrics::Registry::default()
    }
}

impl<T: Store + Send + Sync + Sized> Stage<T> {
    pub fn new(store: T, counter: Counter<u64>) -> (Self, Point) {
        let state = state::State::new(Arc::new(std::sync::Mutex::new(store)));

        let tip = state.tip().into_owned();

        (
            Self {
                upstream: Default::default(),
                state: Arc::new(Mutex::new(state)),
                counter,
            },
            tip,
        )
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl<T: Store + Send + Sync> gasket::framework::Worker<Stage<T>> for Worker {
    async fn bootstrap(_stage: &Stage<T>) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut Stage<T>,
    ) -> Result<WorkSchedule<ValidateHeaderEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;
        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &ValidateHeaderEvent,
        stage: &mut Stage<T>,
    ) -> Result<(), WorkerError> {
        match unit {
            ValidateHeaderEvent::Validated(point, raw_block) => {
                let span_forward = info_span!(
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

                let span_parse_block = info_span!(
                    target: EVENT_TARGET,
                    parent: &span_forward,
                    "parse_block",
                    point = point.slot_or_default(),
                    block.size = raw_block.len(),
                )
                .entered();

                let (block_header_hash, block) = parse_block(&raw_block[..]);

                span_parse_block.exit();

                span_forward.record("header.height", block.header.header_body.block_number);
                span_forward.record("header.slot", block.header.header_body.slot);
                span_forward.record("header.hash", hex::encode(block_header_hash));

                stage.counter.add(1, &[]);

                let mut state = stage.state.lock().await;

                state.forward(&span_forward, point, block).map_err(|e| {
                    error!(target: EVENT_TARGET, error = ?e, "forward.failed");
                    WorkerError::Panic
                })?;

                span_forward.exit();
            }

            ValidateHeaderEvent::Rollback(point) => {
                let span_backward = info_span!(
                    target: EVENT_TARGET,
                    "backward",
                    point.slot = point.slot_or_default(),
                    point.hash = tracing::field::Empty,
                )
                .entered();

                if let Point::Specific(_, header_hash) = point {
                    span_backward.record("point.hash", hex::encode(header_hash));
                }

                let mut state = stage.state.lock().await;

                if let Err(e) = state.backward(point) {
                    match e {
                        BackwardErr::UnknownRollbackPoint(_) => {
                            warn!(target: EVENT_TARGET, "rollback.unknown_point")
                        }
                    }
                }

                span_backward.exit();
            }
        }

        Ok(())
    }
}

fn parse_block(bytes: &[u8]) -> (Hash<32>, MintedBlock<'_>) {
    let (_, block): (u16, MintedBlock<'_>) = cbor::decode(bytes)
        .unwrap_or_else(|_| panic!("failed to decode Conway block: {:?}", hex::encode(bytes)));

    (Hasher::<256>::hash(block.header.raw_cbor()), block)
}
