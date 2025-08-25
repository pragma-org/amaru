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

use crate::{Slot, TransactionPointer, cbor, heterogeneous_array};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProposalPointer {
    pub transaction: TransactionPointer,
    pub proposal_index: usize,
}

impl ProposalPointer {
    pub fn slot(&self) -> Slot {
        self.transaction.slot
    }
}

impl<C> cbor::encode::Encode<C> for ProposalPointer {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.transaction, ctx)?;
        e.encode_with(self.proposal_index, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProposalPointer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(ProposalPointer {
                transaction: d.decode_with(ctx)?,
                proposal_index: d.decode_with(ctx)?,
            })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::{prop_cbor_roundtrip, tests::any_transaction_pointer};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(ProposalPointer, any_proposal_pointer(u64::MAX));

    prop_compose! {
        pub fn any_proposal_pointer(max_slot: u64)(
            transaction in any_transaction_pointer(max_slot),
            proposal_index in any::<usize>(),
        ) -> ProposalPointer {
            ProposalPointer {
                transaction,
                proposal_index,
            }
        }
    }
}
