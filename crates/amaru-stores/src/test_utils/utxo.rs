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

#[cfg(test)]
pub(crate) mod test {
    use amaru_kernel::{
        Bytes, Hash, PostAlonzoTransactionOutput, PseudoTransactionOutput, TransactionInput, Value,
    };
    use proptest::prelude::*;

    prop_compose! {
        pub(crate) fn any_txin()(
            hash in any::<[u8; 32]>(),
            index in any::<u16>(),
        ) -> TransactionInput {
            TransactionInput {
                transaction_id: Hash::from(hash),
                index: index as u64,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_pseudo_transaction_output()(
            amount in any::<u64>(),
        ) -> PseudoTransactionOutput<PostAlonzoTransactionOutput> {
            let inner = PostAlonzoTransactionOutput {
                address: Bytes::from(vec![0u8; 32]),
                value: Value::Coin(amount),
                datum_option: None,
                script_ref: None,
            };

            PseudoTransactionOutput::PostAlonzo(inner)
        }
    }
}
