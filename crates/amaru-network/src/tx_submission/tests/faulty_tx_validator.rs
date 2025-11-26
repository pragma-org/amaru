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

use crate::tx_submission::tests::Tx;
use amaru_ouroboros_traits::{CanValidateTransactions, TransactionValidationError};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// This faulty transaction validator rejects every second transaction.
#[derive(Clone, Debug, Default)]
pub struct FaultyTxValidator {
    count: Arc<Mutex<u16>>,
}

#[async_trait]
impl CanValidateTransactions<Tx> for FaultyTxValidator {
    async fn validate_transaction(&self, _tx: &Tx) -> Result<(), TransactionValidationError> {
        // Reject every second transaction
        let mut count = self.count.lock().await;
        let is_valid = *count % 2 == 0;
        *count += 1;
        if is_valid {
            Ok(())
        } else {
            Err(TransactionValidationError::new(anyhow::anyhow!(
                "Transaction is invalid"
            )))
        }
    }
}
