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

use std::sync::Arc;

use amaru_kernel::{BlockHeader, IgnoreEq, Peer, Point, Tip};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    BlockValidationError, CanValidateBlocks,
    can_validate_blocks::{CanValidateHeaders, HeaderValidationError},
};
use amaru_protocols::store_effects::ResourceHeaderStore;
use opentelemetry::trace::FutureExt;
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, ExternalEffectSync, Resources, SendData};

use crate::errors::{ConsensusError, ValidationFailed};

/// Ledger operations available to a stage.
/// This trait can have mock implementations for unit testing a stage.
pub trait LedgerOps: Send + Sync {
    fn validate_header(
        &self,
        header: &BlockHeader,
        ctx: opentelemetry::Context,
    ) -> BoxFuture<'_, Result<(), HeaderValidationError>>;

    fn validate_block(
        &self,
        peer: &Peer,
        point: &Point,
        ctx: opentelemetry::Context,
    ) -> BoxFuture<'_, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>;

    fn rollback(
        &self,
        peer: &Peer,
        point: &Point,
        ctx: opentelemetry::Context,
    ) -> BoxFuture<'_, anyhow::Result<(), ValidationFailed>>;

    fn contains_point(&self, point: &Point) -> bool;

    fn tip(&self) -> Tip;

    fn volatile_tip(&self) -> Option<Tip>;
}

/// Implementation of LedgerOps using pure_stage::Effects.
pub struct Ledger<T>(Effects<T>);

impl<T> Ledger<T> {
    pub fn new(effects: Effects<T>) -> Ledger<T> {
        Ledger(effects)
    }

    pub fn eff(&self) -> &Effects<T> {
        &self.0
    }
}

impl<T: SendData + Sync> LedgerOps for Ledger<T> {
    fn validate_header(
        &self,
        header: &BlockHeader,
        ctx: opentelemetry::Context,
    ) -> BoxFuture<'_, Result<(), HeaderValidationError>> {
        self.0.external(ValidateHeaderEffect::new(header, ctx))
    }

    fn validate_block(
        &self,
        peer: &Peer,
        point: &Point,
        ctx: opentelemetry::Context,
    ) -> BoxFuture<'_, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>> {
        self.0.external(ValidateBlockEffect::new(peer, point, ctx))
    }

    fn rollback(
        &self,
        peer: &Peer,
        point: &Point,
        ctx: opentelemetry::Context,
    ) -> BoxFuture<'_, anyhow::Result<(), ValidationFailed>> {
        self.0.external(RollbackBlockEffect::new(peer, point, ctx))
    }

    fn contains_point(&self, point: &Point) -> bool {
        self.0.external_sync(ContainsPointEffect::new(point))
    }

    fn tip(&self) -> Tip {
        self.0.external_sync(TipEffect)
    }

    fn volatile_tip(&self) -> Option<Tip> {
        self.0.external_sync(VolatileTipEffect)
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
    #[serde(skip)]
    ctx: IgnoreEq<opentelemetry::Context>,
}

impl ValidateBlockEffect {
    pub fn new(peer: &Peer, point: &Point, ctx: opentelemetry::Context) -> Self {
        Self { peer: peer.clone(), point: *point, ctx: ctx.into() }
    }
}

impl ExternalEffect for ValidateBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let Self { peer: _peer, point, ctx } = *self;
        Self::wrap(
            async move {
                let store = resources
                    .get::<ResourceHeaderStore>()
                    .expect("ValidateBlockEffect requires a ResourceHeaderStore resource")
                    .clone();
                let block = store
                    .load_block(&point.hash())
                    .map_err(|e| BlockValidationError::new(e.into()))?
                    .ok_or(BlockValidationError::new(anyhow::anyhow!("block not found")))?
                    .decode()
                    .map_err(|e| BlockValidationError::new(e.into()))?;
                let validator = resources
                    .get::<ResourceBlockValidation>()
                    .expect("ValidateBlockEffect requires a ResourceBlockValidation resource")
                    .clone();
                validator.roll_forward_block(&point, block).await
            }
            .with_context(ctx.0),
        )
    }
}

impl ExternalEffectAPI for ValidateBlockEffect {
    type Response = Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateHeaderEffect {
    header: BlockHeader,
    #[serde(skip)]
    ctx: IgnoreEq<opentelemetry::Context>,
}

impl ValidateHeaderEffect {
    pub fn new(header: &BlockHeader, ctx: opentelemetry::Context) -> Self {
        Self { header: header.clone(), ctx: ctx.into() }
    }
}

impl ExternalEffect for ValidateHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let _guard = self.ctx.0.attach();
            let validator = resources
                .get::<ResourceHeaderValidation>()
                .expect("ValidateHeaderEffect requires a ResourceHeaderValidation resource")
                .clone();
            validator.validate_header(&self.header)
        })
    }
}

impl ExternalEffectAPI for ValidateHeaderEffect {
    type Response = Result<(), HeaderValidationError>;
}

impl ExternalEffectSync for ValidateHeaderEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RollbackBlockEffect {
    peer: Peer,
    point: Point,
    #[serde(skip)]
    ctx: IgnoreEq<opentelemetry::Context>,
}

impl RollbackBlockEffect {
    pub fn new(peer: &Peer, point: &Point, ctx: opentelemetry::Context) -> Self {
        Self { peer: peer.clone(), point: *point, ctx: ctx.into() }
    }
}

impl ExternalEffect for RollbackBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let _guard = self.ctx.0.attach();
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("RollbackBlockEffect requires a ResourceBlockValidation resource")
                .clone();
            validator
                .rollback_block(&self.point)
                .map_err(|e| ValidationFailed::new(&self.peer, ConsensusError::RollbackBlockFailed(self.point, e)))
        })
    }
}

impl ExternalEffectAPI for RollbackBlockEffect {
    type Response = anyhow::Result<(), ValidationFailed>;
}

impl ExternalEffectSync for RollbackBlockEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContainsPointEffect {
    point: Point,
}

impl ContainsPointEffect {
    pub fn new(point: &Point) -> Self {
        Self { point: *point }
    }
}

impl ExternalEffect for ContainsPointEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        #[expect(clippy::expect_used)]
        Self::wrap_sync({
            let ledger = resources
                .get::<ResourceBlockValidation>()
                .expect("ContainsPointEffect requires a ResourceBlockValidation resource")
                .clone();
            ledger.contains_point(&self.point)
        })
    }
}

impl ExternalEffectAPI for ContainsPointEffect {
    type Response = bool;
}

impl ExternalEffectSync for ContainsPointEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TipEffect;

impl ExternalEffect for TipEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let ledger = resources
                .get::<ResourceBlockValidation>()
                .expect("TipEffect requires a ResourceBlockValidation resource")
                .clone();
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("TipEffect requires a ResourceHeaderStore resource")
                .clone();
            let point = ledger.tip();
            store.load_tip(&point.hash()).expect("cannot load header for ledger tip")
        })
    }
}

impl ExternalEffectAPI for TipEffect {
    type Response = Tip;
}

impl ExternalEffectSync for TipEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VolatileTipEffect;

impl ExternalEffect for VolatileTipEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let ledger = resources
                .get::<ResourceBlockValidation>()
                .expect("VolatileTipPointEffect requires a ResourceBlockValidation resource")
                .clone();
            ledger.volatile_tip()
        })
    }
}

impl ExternalEffectAPI for VolatileTipEffect {
    type Response = Option<Tip>;
}

impl ExternalEffectSync for VolatileTipEffect {}
