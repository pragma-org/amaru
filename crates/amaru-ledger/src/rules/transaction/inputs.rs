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

use amaru_kernel::{MintedTransactionBody, TransactionInput};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidInputs {
    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        intersection
            .iter()
            .map(|input|
                format!("{}#{}", input.transaction_id, input.transaction_id)
            )
            .collect::<Vec<_>>()
            .join(", ")
    )]
    NonDisjointRefInputs { intersection: Vec<TransactionInput> },
}

pub fn execute(transaction: &MintedTransactionBody<'_>) -> Result<(), InvalidInputs> {
    let intersection = match &transaction.reference_inputs {
        Some(ref_inputs) => ref_inputs
            .iter()
            .filter(|ref_input| transaction.inputs.contains(ref_input))
            .cloned()
            .collect(),
        None => Vec::new(),
    };

    if !intersection.is_empty() {
        Err(InvalidInputs::NonDisjointRefInputs { intersection })
    } else {
        Ok(())
    }
}
