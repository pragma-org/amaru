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

use crate::{Slot, cbor, heterogeneous_array};
use std::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct TransactionPointer {
    pub slot: Slot,
    pub transaction_index: usize,
}

impl fmt::Display for TransactionPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "slot={},transaction={}",
            &self.slot, &self.transaction_index
        )
    }
}

impl<C> cbor::encode::Encode<C> for TransactionPointer {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.slot, ctx)?;
        e.encode_with(self.transaction_index, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for TransactionPointer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(TransactionPointer {
                slot: d.decode_with(ctx)?,
                transaction_index: d.decode_with(ctx)?,
            })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::{Slot, prop_cbor_roundtrip};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(TransactionPointer, any_transaction_pointer(u64::MAX));

    prop_compose! {
        pub fn any_transaction_pointer(max_slot: u64)(
            slot in 0..max_slot,
            transaction_index in any::<usize>(),
        ) -> TransactionPointer {
            TransactionPointer {
                slot: Slot::from(slot),
                transaction_index,
            }
        }
    }
}
