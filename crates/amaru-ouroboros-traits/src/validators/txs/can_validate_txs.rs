// Copyright 2026 PRAGMA
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

use amaru_kernel::Transaction;
use async_trait::async_trait;

use crate::mempool::TransactionValidationError;

pub type ResourceTxValidation<State = ()> = Arc<dyn CanValidateTxs<State>>;

/// Selects how much work a transaction validator performs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ValidationMode {
    /// Validate all transaction witnesses and execute phase-two scripts.
    #[default]
    FullValidation,
    /// Re-check state-dependent phase-one rules using the result of an earlier full validation.
    Revalidation,
}

/// This trait abstract over the possibility to validate transactions.
/// Concretely speaking this will be done by the ledger.
#[async_trait]
pub trait CanValidateTxs<State = ()>: Send + Sync
where
    State: Send + Sync + 'static,
{
    async fn validate_tx(
        &self,
        tx: &Transaction,
        previous: &[State],
        mode: ValidationMode,
    ) -> Result<State, TransactionValidationError>;
}

#[async_trait]
impl<F, State> CanValidateTxs<State> for F
where
    F: Fn(&Transaction, &[State]) -> Result<State, TransactionValidationError> + Send + Sync,
    State: Send + Sync + 'static,
{
    async fn validate_tx(
        &self,
        tx: &Transaction,
        previous: &[State],
        _mode: ValidationMode,
    ) -> Result<State, TransactionValidationError> {
        self(tx, previous)
    }
}

/// A fake transaction validator that always returns ok.
#[derive(Clone, Debug, Default)]
pub struct MockCanValidateTxs;

#[async_trait]
impl<State: Default + Send + Sync + 'static> CanValidateTxs<State> for MockCanValidateTxs {
    async fn validate_tx(
        &self,
        _tx: &Transaction,
        _previous: &[State],
        _mode: ValidationMode,
    ) -> Result<State, TransactionValidationError> {
        Ok(State::default())
    }
}
