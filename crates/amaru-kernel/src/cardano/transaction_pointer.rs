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

use std::fmt;

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy, any};

use crate::{Slot, cbor};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TransactionPointer {
    pub slot: Slot,
    pub transaction_index: usize,
}

impl fmt::Display for TransactionPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "slot={},transaction={}", self.slot, self.transaction_index)
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
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(TransactionPointer { slot: d.decode_with(ctx)?, transaction_index: d.decode_with(ctx)? })
        })
    }
}

/// Exclusive upper bound on the slots an `Arbitrary` impl draws. Defaults to the whole slot range,
/// so `any::<T>()` places no restriction.
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotUpperBound(pub Slot);

#[cfg(any(test, feature = "test-utils"))]
impl Default for SlotUpperBound {
    fn default() -> Self {
        Self(Slot::new(u64::MAX))
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for TransactionPointer {
    type Parameters = SlotUpperBound;
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(SlotUpperBound(max_slot): Self::Parameters) -> Self::Strategy {
        (0..max_slot.as_u64(), any::<usize>())
            .prop_map(|(slot, transaction_index)| TransactionPointer { slot: Slot::from(slot), transaction_index })
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::TransactionPointer;
    use crate::prop_cbor_roundtrip;

    prop_cbor_roundtrip!(TransactionPointer);
}
