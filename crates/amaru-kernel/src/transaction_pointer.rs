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

use crate::{cbor, heterogeneous_array, Slot};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct TransactionPointer {
    pub slot: Slot,
    pub transaction_index: usize,
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
        heterogeneous_array(d, 2, |d| {
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
    use crate::{prop_cbor_roundtrip, Slot};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(TransactionPointer, any_transaction_pointer());

    prop_compose! {
        pub fn any_transaction_pointer()(
            slot in 0..10_000_000u64,
            transaction_index in any::<usize>(),
        ) -> TransactionPointer {
            TransactionPointer {
                slot: Slot::from(slot),
                transaction_index,
            }
        }
    }
}
