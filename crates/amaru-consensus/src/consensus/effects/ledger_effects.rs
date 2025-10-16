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
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::can_validate_blocks::{CanValidateHeaders, HeaderValidationError};
use amaru_ouroboros_traits::{BlockHeader, BlockValidationError, CanValidateBlocks};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use std::sync::Arc;

/// Ledger operations available to a stage.
/// This trait can have mock implementations for unit testing a stage.
pub trait LedgerOps: Send + Sync {
    fn validate_header(
        &self,
        point: &Point,
        header: &BlockHeader,
    ) -> BoxFuture<'_, Result<(), HeaderValidationError>>;

    fn validate_block(
        &self,
        peer: &Peer,
        point: &Point,
        block: RawBlock,
    ) -> BoxFuture<'_, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>;

    fn rollback(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> BoxFuture<'_, anyhow::Result<(), ProcessingFailed>>;
}

/// Implementation of LedgerOps using pure_stage::Effects.
pub struct Ledger<T>(Effects<T>);

impl<T> Ledger<T> {
    pub fn new(effects: Effects<T>) -> Ledger<T> {
        Ledger(effects)
    }
}

impl<T: SendData + Sync> LedgerOps for Ledger<T> {
    fn validate_header(
        &self,
        point: &Point,
        header: &BlockHeader,
    ) -> BoxFuture<'_, Result<(), HeaderValidationError>> {
        self.0.external(ValidateHeaderEffect::new(point, header))
    }

    fn validate_block(
        &self,
        peer: &Peer,
        point: &Point,
        block: RawBlock,
    ) -> BoxFuture<'_, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>
    {
        self.0
            .external(ValidateBlockEffect::new(peer, point, block))
    }

    fn rollback(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> BoxFuture<'_, anyhow::Result<(), ProcessingFailed>> {
        self.0.external(RollbackBlockEffect::new(peer, point))
    }
}

// EXTERNAL EFFECTS DEFINITIONS

/// Resource types for ledger operations.
pub type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;
pub type ResourceHeaderValidation = Arc<dyn CanValidateHeaders + Send + Sync>;

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
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a ResourceBlockValidation resource")
                .clone();
            validator.roll_forward_block(&self.point, &self.block).await
        })
    }
}

impl ExternalEffectAPI for ValidateBlockEffect {
    type Response = Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateHeaderEffect {
    point: Point,
    header: BlockHeader,
}

impl ValidateHeaderEffect {
    pub fn new(point: &Point, header: &BlockHeader) -> Self {
        Self {
            point: point.clone(),
            header: header.clone(),
        }
    }
}

impl ExternalEffect for ValidateHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let validator = resources
                .get::<ResourceHeaderValidation>()
                .expect("ValidateHeaderEffect requires a ResourceHeaderValidation resource")
                .clone();
            validator.validate_header(&self.point, &self.header)
        })
    }
}

impl ExternalEffectAPI for ValidateHeaderEffect {
    type Response = Result<(), HeaderValidationError>;
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
        Self::wrap(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("RollbackBlockEffect requires a ResourceBlockValidation resource")
                .clone();
            validator
                .rollback_block(&self.point)
                .map_err(|e| ProcessingFailed::new(&self.peer, e.to_anyhow()))
        })
    }
}

impl ExternalEffectAPI for RollbackBlockEffect {
    type Response = anyhow::Result<(), ProcessingFailed>;
}
