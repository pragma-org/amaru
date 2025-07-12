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

use amaru_kernel::{MemoizedTransactionOutput, TransactionInput};
use iter_borrow::IterBorrow;

pub type Key = TransactionInput;

pub type Value = MemoizedTransactionOutput;

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Value>>;

#[cfg(any(test, feature = "test-utils"))]
use amaru_kernel::{
    Bytes, Hash, PostAlonzoTransactionOutput, PseudoTransactionOutput, Value as KernelValue,
};
#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::*;

#[cfg(any(test, feature = "test-utils"))]
prop_compose! {
    pub fn any_txin()(
        hash in any::<[u8; 32]>(),
        index in any::<u16>(),
    ) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash::from(hash),
            index: index as u64,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub fn any_pseudo_transaction_output(
) -> impl Strategy<Value = PseudoTransactionOutput<PostAlonzoTransactionOutput>> {
    any::<u64>().prop_map(|amount| {
        let inner = PostAlonzoTransactionOutput {
            address: Bytes::from(vec![0u8; 32]),
            value: KernelValue::Coin(amount),
            datum_option: None,
            script_ref: None,
        };
        PseudoTransactionOutput::PostAlonzo(inner)
    })
}
