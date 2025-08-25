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

use amaru_iter_borrow::IterBorrow;
use amaru_kernel::{ComparableProposalId, Proposal, ProposalPointer, cbor};
use amaru_slot_arithmetic::Epoch;

pub const EVENT_TARGET: &str = "amaru::ledger::store::proposals";

/// Iterator used to browse rows from the proposals column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Key = ComparableProposalId;

pub type Value = Row;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub proposed_in: ProposalPointer,
    pub valid_until: Epoch,
    pub proposal: Proposal,
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.encode_with(self.proposed_in, ctx)?;
        e.encode_with(self.valid_until, ctx)?;
        e.encode_with(&self.proposal, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            proposed_in: d.decode_with(ctx)?,
            valid_until: d.decode_with(ctx)?,
            proposal: d.decode_with(ctx)?,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use amaru_kernel::{
        prop_cbor_roundtrip,
        tests::{any_proposal, any_proposal_pointer},
    };
    use proptest::{prelude::*, prop_compose};

    #[cfg(not(target_os = "windows"))]
    prop_cbor_roundtrip!(prop_cbor_roundtrip_row, Row, any_row(u64::MAX));

    prop_compose! {
        pub fn any_row(max_slot: u64)(
            proposed_in in any_proposal_pointer(max_slot),
            valid_until in any::<Epoch>(),
            proposal in any_proposal(),
        ) -> Row {
            Row {
                proposed_in,
                valid_until,
                proposal,
            }
        }
    }
}
