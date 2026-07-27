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

pub use pallas_primitives::TransactionInput;

pub fn transaction_input_to_string(input: &TransactionInput) -> String {
    format!("{}#{}", input.transaction_id, input.index)
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::TransactionInput;
    use crate::any_hash32;

    prop_compose! {
        pub fn any_transaction_input()(
            id in any_hash32(),
            ix in any::<u64>(),
        ) -> TransactionInput {
            TransactionInput { transaction_id: id, index: ix }
        }
    }
}
