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

use std::{collections::BTreeSet, sync::Arc};

use amaru_kernel::{ConsensusParameters, EraHistory, Header, PeerCandidate, Point, Transaction};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::TraceContext;
use amaru_ouroboros_traits::{
    BlockValidationError, CanValidateBlocks, CanValidateTxs, FindCommonAncestorResult, ForkSwitchOutcome,
    HasStakePools, Nonces, PoolSummaries, TransactionValidationError,
};
use amaru_protocols::store_effects::ResourceHeaderStore;
use amaru_pure_stage::{BoxFuture, Effects, ExternalEffectAPI, Resources, SendData, Void};
use anyhow::anyhow;
use opentelemetry::trace::FutureExt;

use crate::validate_header::ValidateHeaderError;

/// Ledger operations available to a stage.
/// This trait can have mock implementations for unit testing a stage.
pub trait LedgerOps: Send + Sync {
    fn validate_tx(&self, tx: &Transaction) -> BoxFuture<'_, Result<(), TransactionValidationError>>;

    /// Validate a header and return its evolved nonces, which the caller is expected to store
    /// atomically with the header itself.
    fn validate_header(&self, header: &Header) -> BoxFuture<'static, Result<Nonces, ValidateHeaderError>>;

    fn validate_block(
        &self,
        point: &Point,
    ) -> BoxFuture<'static, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>;

    fn switch_to_fork(&self, tip: &Point) -> BoxFuture<'static, Result<ForkSwitchOutcome, BlockValidationError>>;

    fn immutable_tip(&self) -> BoxFuture<'static, Point>;

    fn volatile_tip(&self) -> BoxFuture<'static, Point>;

    /// Get registered relay candidates from the stable store.
    ///
    /// Entries may be socket addresses, hostnames, or SRV names; they are not
    /// necessarily dialable until resolved.
    ///
    /// **NOTE:** This operation blocks the ledger for about 4ms (mainnet late
    /// 2025), so it should be called with care. Please cache the result, it
    /// only changes meaningfully once per epoch.
    fn registered_relay_candidates(&self) -> BoxFuture<'_, Result<BTreeSet<PeerCandidate>, BlockValidationError>>;
}

/// Implementation of LedgerOps using amaru_pure_stage::Effects.
#[derive(Clone, Debug)]
pub struct Ledger {
    effects: Effects<Void>,
    trace_context: TraceContext,
}

impl Ledger {
    pub fn new<T: SendData>(effects: Effects<T>) -> Self {
        Self { effects: effects.erase(), trace_context: Default::default() }
    }

    pub fn with_trace_context(mut self, trace_context: &TraceContext) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

impl LedgerOps for Ledger {
    fn validate_tx(&self, tx: &Transaction) -> BoxFuture<'_, Result<(), TransactionValidationError>> {
        self.effects.external(ValidateTxEffect::new(tx))
    }

    fn validate_header(&self, header: &Header) -> BoxFuture<'static, Result<Nonces, ValidateHeaderError>> {
        self.effects.external(ValidateHeaderEffect::new(header).with_trace_context(&self.trace_context))
    }

    fn validate_block(
        &self,
        point: &Point,
    ) -> BoxFuture<'static, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>> {
        self.effects.external(ValidateBlockEffect::new(point).with_trace_context(&self.trace_context))
    }

    fn switch_to_fork(&self, tip: &Point) -> BoxFuture<'static, Result<ForkSwitchOutcome, BlockValidationError>> {
        self.effects.external(SwitchToForkEffect::new(tip).with_trace_context(&self.trace_context))
    }

    fn immutable_tip(&self) -> BoxFuture<'static, Point> {
        self.effects.external(TipEffect)
    }

    fn volatile_tip(&self) -> BoxFuture<'static, Point> {
        self.effects.external(VolatileTipEffect)
    }

    fn registered_relay_candidates(&self) -> BoxFuture<'_, Result<BTreeSet<PeerCandidate>, BlockValidationError>> {
        self.effects.external(RegisteredRelayCandidatesEffect)
    }
}

// EXTERNAL EFFECTS DEFINITIONS

/// Resource types for ledger operations.
pub type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;
pub type ResourceTxValidation = Arc<dyn CanValidateTxs + Send + Sync>;
pub type ResourceHasStakePools = Arc<dyn HasStakePools + Send + Sync>;
pub type ResourceEraHistory = EraHistory;
pub type ResourceConsensusParameters = Arc<ConsensusParameters>;
pub type ResourcePoolSummaries = Arc<PoolSummaries>;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ValidateTxEffect {
    tx: Transaction,
}

impl ValidateTxEffect {
    pub fn new(tx: &Transaction) -> Self {
        Self { tx: tx.clone() }
    }
}

impl PartialEq for ValidateTxEffect {
    fn eq(&self, other: &Self) -> bool {
        self.tx == other.tx
    }
}

impl ExternalEffectAPI for ValidateTxEffect {
    type Response = Result<(), TransactionValidationError>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let validator = resources
                .get::<ResourceTxValidation>()
                .expect("ValidateTxEffect requires a ResourceTxValidation resource")
                .clone();
            validator.validate_tx(&self.tx)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockEffect {
    point: Point,
    trace_context: TraceContext,
}

impl ValidateBlockEffect {
    pub fn new(point: &Point) -> Self {
        Self { point: *point, trace_context: Default::default() }
    }

    pub fn with_trace_context(mut self, trace_context: &TraceContext) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

impl ExternalEffectAPI for ValidateBlockEffect {
    type Response = Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| {
            let Self { point, trace_context } = this;
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
                validator.roll_forward_block(block).await
            }
            .with_context(trace_context.context())
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateHeaderEffect {
    header: Header,
    trace_context: TraceContext,
}

impl ValidateHeaderEffect {
    pub fn new(header: &Header) -> Self {
        Self { header: header.clone(), trace_context: Default::default() }
    }

    pub fn with_trace_context(mut self, trace_context: &TraceContext) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

impl ExternalEffectAPI for ValidateHeaderEffect {
    type Response = Result<Nonces, ValidateHeaderError>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let _guard = self.trace_context.attach();

            let consensus_parameters = resources
                .get::<ResourceConsensusParameters>()
                .expect("ValidateHeaderEffect requires a ResourceConsensusParameters resource")
                .clone();
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("ValidateHeaderEffect requires a ResourceHeaderStore resource")
                .clone();
            let pool_summaries = resources
                .get::<ResourcePoolSummaries>()
                .expect("ValidateHeaderEffect requires a ResourcePoolSummaries resource")
                .clone();
            let era_history = resources
                .get::<ResourceEraHistory>()
                .expect("ValidateHeaderEffect requires a ResourceEraHistory resource")
                .clone();

            crate::validate_header::validate_header(
                &self.header,
                consensus_parameters,
                store,
                pool_summaries,
                Arc::new(era_history),
            )
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SwitchToForkEffect {
    tip: Point,
    trace_context: TraceContext,
}

impl SwitchToForkEffect {
    pub fn new(tip: &Point) -> Self {
        Self { tip: *tip, trace_context: Default::default() }
    }

    pub fn with_trace_context(mut self, trace_context: &TraceContext) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

impl ExternalEffectAPI for SwitchToForkEffect {
    type Response = Result<ForkSwitchOutcome, BlockValidationError>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| {
            let Self { tip, trace_context } = this;
            async move {
                let store = resources
                    .get::<ResourceHeaderStore>()
                    .expect("SwitchToForkEffect requires a ResourceHeaderStore resource")
                    .clone();
                let validator = resources
                    .get::<ResourceBlockValidation>()
                    .expect("SwitchToForkEffect requires a ResourceBlockValidation resource")
                    .clone();

                // Find the intersection with the current ledger tip
                let ledger_tip = validator.tip();
                let tip_hash = tip.hash();

                let fork_point = match store
                    .find_common_ancestor(tip.hash(), ledger_tip.hash())
                    .map_err(|error| BlockValidationError::new(anyhow!(error)))?
                {
                    FindCommonAncestorResult::Found(point) => point,
                    FindCommonAncestorResult::HeaderNotFound(hash) => {
                        return Err(BlockValidationError::new(anyhow!("header missing for {hash}")));
                    }
                    FindCommonAncestorResult::NotFound => {
                        return Err(BlockValidationError::new(anyhow!(
                            "no common ancestor between the ledger tip {ledger_tip} and {tip_hash}"
                        )));
                    }
                };

                validator.switch_to_fork(&fork_point, &tip)
            }
            .with_context(trace_context.context())
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TipEffect;

impl ExternalEffectAPI for TipEffect {
    type Response = Point;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let ledger = resources
                .get::<ResourceBlockValidation>()
                .expect("TipEffect requires a ResourceBlockValidation resource")
                .clone();
            ledger.tip()
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VolatileTipEffect;

impl ExternalEffectAPI for VolatileTipEffect {
    type Response = Point;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let ledger = resources
                .get::<ResourceBlockValidation>()
                .expect("VolatileTipPointEffect requires a ResourceBlockValidation resource")
                .clone();
            ledger.volatile_tip().unwrap_or_else(|| ledger.tip())
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegisteredRelayCandidatesEffect;

impl ExternalEffectAPI for RegisteredRelayCandidatesEffect {
    type Response = Result<BTreeSet<PeerCandidate>, BlockValidationError>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let stake_pools = resources
                .get::<ResourceHasStakePools>()
                .expect("RegisteredRelayCandidatesEffect requires a ResourceHasStakePools resource")
                .clone();
            stake_pools.registered_relay_candidates()
        })
    }
}
