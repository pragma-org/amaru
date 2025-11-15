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

#![feature(try_trait_v2)]
#![feature(btree_set_entry)]

use std::{borrow::Borrow, fmt, sync::Arc};

pub mod block_validator;
pub mod bootstrap;
pub mod context;
pub mod governance;
pub mod rules;
pub mod state;
pub mod store;
pub mod summary;

// TODO: Move to its own crate / module.
/// A mapping from an Arc-owned type to another. The transformation must be fully known at
/// compile-time to allow for the entire structure to be cloned.
///
/// Also, to ease the borrow-checker, it is required that `A` and `B` are also plain types (or
/// statically borrowed references) but cannot themselves hold any lifetime shorter than 'static.
#[derive(Clone)]
pub struct ArcMapped<A: 'static, B: 'static> {
    arc: Arc<A>,
    transform: &'static fn(&A) -> &B,
}

impl<A, B: fmt::Debug> fmt::Debug for ArcMapped<A, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let b: &B = self.borrow();
        b.fmt(f)
    }
}

impl<A: 'static, B: 'static> ArcMapped<A, B> {
    fn new(arc: Arc<A>, transform: &'static fn(&A) -> &B) -> Self {
        Self { arc, transform }
    }
}

/// Obtain a handle on `B` by simply applying the transformation function on the borrowed Arc<A>
impl<A, B> std::borrow::Borrow<B> for ArcMapped<A, B> {
    fn borrow(&self) -> &B {
        let f = &self.transform;
        f(self.arc.as_ref())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::{
        Bytes, Hash, MemoizedTransactionOutput, PostAlonzoTransactionOutput, TransactionInput,
        TransactionOutput, Value, from_cbor, to_cbor,
    };

    pub(crate) fn fake_input(transaction_id: &str, index: u64) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash::from(hex::decode(transaction_id).unwrap().as_slice()),
            index,
        }
    }

    pub(crate) fn fake_output(address: &str) -> MemoizedTransactionOutput {
        let output = TransactionOutput::PostAlonzo(PostAlonzoTransactionOutput {
            address: Bytes::from(hex::decode(address).expect("Invalid hex address")),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        });

        from_cbor(&to_cbor(&output)).unwrap()
    }
}
