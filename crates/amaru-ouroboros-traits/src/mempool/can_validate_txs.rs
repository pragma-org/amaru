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

use amaru_kernel::Transaction;

use crate::mempool::TransactionValidationError;

pub type ResourceTxValidation<Tx> = Arc<dyn CanValidateTxs<Tx>>;

/// This trait abstract over the possibility to validate transactions.
/// Concretely speaking this will be done by the ledger.
pub trait CanValidateTxs<Tx: Send + Sync + 'static>: Send + Sync {
    fn validate_tx(&self, tx: &Tx) -> Result<(), TransactionValidationError>;
}

/// A simple function can be used to implement the validation trait
impl<Tx, F> CanValidateTxs<Tx> for F
where
    Tx: Send + Sync + 'static,
    F: Fn(&Tx) -> Result<(), TransactionValidationError> + Send + Sync,
{
    fn validate_tx(&self, tx: &Tx) -> Result<(), TransactionValidationError> {
        self(tx)
    }
}

/// A fake transaction validator that always returns ok.
#[derive(Clone, Debug, Default)]
pub struct MockCanValidateTxs;
impl CanValidateTxs<Transaction> for MockCanValidateTxs {
    fn validate_tx(&self, _tx: &Transaction) -> Result<(), TransactionValidationError> {
        Ok(())
    }
}
