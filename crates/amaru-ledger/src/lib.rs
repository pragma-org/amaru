// Copyright 2024 PRAGMA
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

// EDR-010 - Ledger Validation Context
// <https://github.com/pragma-org/amaru/blob/main/engineering-decision-records/010-ledger-validation-context.md>
#![feature(try_trait_v2)]

pub mod block_validator;
pub mod bootstrap;
pub mod context;
pub mod governance;
pub mod rules;
pub mod state;
pub mod store;
pub mod summary;

#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::{Address, Hash, MemoizedTransactionOutput, TransactionInput, Value};

    pub(crate) fn fake_input(transaction_id: &str, index: u64) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash::from(hex::decode(transaction_id).unwrap().as_slice()),
            index,
        }
    }

    pub(crate) fn fake_output(address: &str) -> MemoizedTransactionOutput {
        MemoizedTransactionOutput {
            is_legacy: false,
            address: Address::from_hex(address).expect("Invalid hex address"),
            value: Value::Coin(0),
            datum: amaru_kernel::MemoizedDatum::None,
            script: None,
        }
    }
}
