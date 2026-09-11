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

use std::fmt;

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy, any};

use crate::{Hash, cbor, hash};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    std::hash::Hash,
    cbor::Encode,
    cbor::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct TransactionInput {
    #[n(0)]
    pub transaction_id: Hash<{ hash::size::TRANSACTION_BODY }>,

    #[n(1)]
    pub index: u64,
}

impl fmt::Display for TransactionInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.transaction_id, self.index)
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for TransactionInput {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (any::<Hash<{ hash::size::TRANSACTION_BODY }>>(), any::<u64>())
            .prop_map(|(transaction_id, index)| TransactionInput { transaction_id, index })
            .boxed()
    }
}
