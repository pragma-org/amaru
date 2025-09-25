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

use crate::consensus::errors::ProcessingFailed;
use amaru_kernel::peer::Peer;
use amaru_kernel::{Point, RawBlock};
use amaru_ouroboros_traits::{BlockValidationError, CanValidateBlocks, HasStakeDistribution};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use std::sync::Arc;
use amaru_metrics::ledger::LedgerMetrics;

pub struct Ledger<'a, T>(&'a Effects<T>);

impl<'a, T> Ledger<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Ledger<'a, T> {
        Ledger(eff)
    }
}

pub trait LedgerOps {
    fn validate(
        &self,
        peer: &Peer,
        point: &Point,
        block: RawBlock,
    ) -> impl Future<Output=Result<Result<u64, BlockValidationError>, BlockValidationError>> + Send;

    fn rollback(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> impl Future<Output=anyhow::Result<(), ProcessingFailed>> + Send;
}

impl<T: SendData + Sync> LedgerOps for Ledger<'_, T> {
    fn validate(
        &self,
        peer: &Peer,
        point: &Point,
        block: RawBlock,
    ) -> impl Future<Output=Result<Result<u64, BlockValidationError>, BlockValidationError>> + Send
    {
        self.0
            .external(ValidateBlockEffect::new(peer, point, block))
    }

    fn rollback(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> impl Future<Output=anyhow::Result<(), ProcessingFailed>> + Send {
        self.0.external(RollbackBlockEffect::new(peer, point))
    }
}

pub type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;
pub type ResourceHeaderValidation = Arc<dyn HasStakeDistribution>;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockEffect {
    peer: Peer,
    point: Point,
    block: RawBlock,
}

impl ValidateBlockEffect {
    pub fn new(peer: &Peer, point: &Point, block: RawBlock) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
            block,
        }
    }
}

impl ExternalEffect for ValidateBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a CanValidateBlock resource")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response =
                validator.roll_forward_block(&self.point, &self.block);
            Box::new(result) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for ValidateBlockEffect {
    type Response = Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RollbackBlockEffect {
    peer: Peer,
    point: Point,
}

impl RollbackBlockEffect {
    pub fn new(peer: &Peer, point: &Point) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
        }
    }
}

impl ExternalEffect for RollbackBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a HasBlockValidation resource")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response = validator
                .rollback_block(&self.point)
                .map_err(|e| ProcessingFailed::new(&self.peer, e.to_anyhow()));
            Box::new(result) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for RollbackBlockEffect {
    type Response = anyhow::Result<(), ProcessingFailed>;
}
