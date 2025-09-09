// Copyright 2025 PRAGMA
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

use crate::state;
use crate::store::{HistoricalStores, Store};
use amaru_kernel::block::StageError;
use amaru_kernel::span::adopt_current_span;
use amaru_kernel::{
    EraHistory, Point, RawBlock,
    block::{BlockValidationResult, ValidateBlockEvent},
    network::NetworkName,
    protocol_parameters::GlobalParameters,
};
use amaru_ouroboros_traits::{HasBlockValidation, IsHeader};
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources, StageRef};
use std::sync::{Arc, Mutex};
use tracing::{Level, error, instrument};

pub struct BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    pub state: Arc<Mutex<state::State<S, HS>>>,
}

impl<S: Store + Send, HS: HistoricalStores + Send> BlockValidator<S, HS> {
    pub fn new(
        store: S,
        snapshots: HS,
        network: NetworkName,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
    ) -> anyhow::Result<Self> {
        let state = state::State::new(store, snapshots, network, era_history, global_parameters)?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
        })
    }

    #[expect(clippy::unwrap_used)]
    pub fn get_tip(&self) -> Point {
        let state = self.state.lock().unwrap();
        state.tip().into_owned()
    }
}

impl<S, HS> HasBlockValidation for BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    #[expect(clippy::unwrap_used)]
    fn roll_forward_block(
        &self,
        point: &Point,
        raw_block: &RawBlock,
    ) -> Result<Result<u64, StageError>, StageError> {
        let mut state = self.state.lock().unwrap();
        state.roll_forward(point, raw_block)
    }

    #[expect(clippy::unwrap_used)]
    fn rollback_block(&self, to: &Point) -> Result<(), StageError> {
        let mut state = self.state.lock().unwrap();
        state.rollback_to(to)
    }
}

type State = (StageRef<BlockValidationResult>, StageRef<StageError>);

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.ledger"
)]
pub async fn stage(
    (downstream, errors): State,
    msg: ValidateBlockEvent,
    eff: Effects<ValidateBlockEvent>,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ValidateBlockEvent::Validated {
            header,
            block,
            span,
            peer,
            ..
        } => {
            let point = header.point();

            match eff
                .external(ValidateBlockEffect::new(&point, block.clone()))
                .await
            {
                Ok(Ok(block_height)) => {
                    eff.send(
                        &downstream,
                        BlockValidationResult::BlockValidated {
                            peer,
                            header,
                            block,
                            span: span.clone(),
                            block_height,
                        },
                    )
                    .await
                }
                Ok(Err(err)) => {
                    error!(?err, "Failed to validate a block");
                    eff.send(&errors, err).await;
                }
                Err(err) => {
                    error!(?err, "Failed to roll forward block");
                    eff.send(&errors, err).await;
                }
            }
        }
        ValidateBlockEvent::Rollback {
            peer,
            rollback_point,
            span,
            ..
        } => {
            if let Err(err) = eff
                .external(RollbackBlockEffect::new(&rollback_point))
                .await
            {
                error!(?err, "Failed to rollback");
                eff.send(&errors, err).await;
            } else {
                eff.send(
                    &downstream,
                    BlockValidationResult::RolledBackTo {
                        peer,
                        rollback_point,
                        span,
                    },
                )
                .await
            }
        }
    };
    (downstream, errors)
}

pub type ResourceBlockValidation = Arc<dyn HasBlockValidation + Send + Sync>;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockEffect {
    point: Point,
    block: RawBlock,
}

impl ValidateBlockEffect {
    pub fn new(point: &Point, block: RawBlock) -> Self {
        Self {
            point: point.clone(),
            block: block.clone(),
        }
    }
}

impl ExternalEffect for ValidateBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a HasBlockValidation resource")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response =
                validator.roll_forward_block(&self.point, &self.block);
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for ValidateBlockEffect {
    type Response = Result<Result<u64, StageError>, StageError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RollbackBlockEffect {
    point: Point,
}

impl RollbackBlockEffect {
    pub fn new(point: &Point) -> Self {
        Self {
            point: point.clone(),
        }
    }
}

impl ExternalEffect for RollbackBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a HasBlockValidation resource")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response =
                validator.rollback_block(&self.point);
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for RollbackBlockEffect {
    type Response = anyhow::Result<(), StageError>;
}
