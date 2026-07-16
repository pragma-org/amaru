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
#![feature(try_trait_v2, try_trait_v2_residual)]

pub mod block_validator;
pub mod bootstrap;
pub mod context;
pub mod epoch_transition;
pub mod governance;
pub mod registered_relay_addrs;
pub mod rules;
pub mod snapshot;
pub mod state;
pub mod store;
pub mod summary;

#[macro_export]
macro_rules! tracing_enabled {
    ($level:expr $(,)?) => {
        tracing::enabled!(target: "amaru::ledger", $level)
    };
}

#[macro_export]
macro_rules! trace {
    ($name:literal $(, $($rest:tt)+)?) => {
        amaru_observability::trace!(target: "amaru::ledger", name: $name $(, $($rest)+)?);
    };
}

#[macro_export]
macro_rules! debug {
    ($name:literal $(, $($rest:tt)+)?) => {
        amaru_observability::debug!(target: "amaru::ledger", name: $name $(, $($rest)+)?);
    };
}

#[macro_export]
macro_rules! info {
    ($name:literal $(, $($rest:tt)+)?) => {
        amaru_observability::info!(target: "amaru::ledger", name: $name $(, $($rest)+)?);
    };
}

#[macro_export]
macro_rules! warn {
    ($name: literal $(, $($rest:tt)+)?) => {
        amaru_observability::warn!(target: "amaru::ledger", name: $name $(, $($rest)+)?);
    };
}

#[macro_export]
macro_rules! error {
    ($name:literal $(, $($rest:tt)+)?) => {
        amaru_observability::error!(target: "amaru::ledger", name: $name $(, $($rest)+)?);
    };
}

#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::{Address, Hash, MemoizedTransactionOutput, MemoizedValue, TransactionInput, Value};

    pub(crate) fn fake_input(transaction_id: &str, index: u64) -> TransactionInput {
        TransactionInput { transaction_id: Hash::from(hex::decode(transaction_id).unwrap().as_slice()), index }
    }

    pub(crate) fn fake_output(address: &str) -> MemoizedTransactionOutput {
        MemoizedTransactionOutput::new(
            false,
            Address::from_hex(address).expect("Invalid hex address"),
            MemoizedValue::new(Value::Coin(0)).expect("Value encoding should never fail"),
            amaru_kernel::MemoizedDatum::None,
            None,
        )
    }
}
