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
use amaru_kernel::{ConstitutionalCommitteeMemberStatus, Epoch, StakeCredential, cbor};

use crate::state::volatile::Resettable;

/// Iterator used to browse rows from the CC members column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (Resettable<ConstitutionalCommitteeMemberStatus>, Resettable<Epoch>);

pub type Key = StakeCredential;

/// What a cold credential currently holds. Existence is not membership: a credential named in an
/// in-flight `UpdateCommittee` may authorize a hot credential before that proposal is enacted, and
/// holds no term until it is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row {
    pub status: Option<ConstitutionalCommitteeMemberStatus>,
    pub valid_until: Option<Epoch>,
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.status.as_ref(), ctx)?;
        e.encode_with(self.valid_until, ctx)?;
        Ok(())
    }
}

impl<'a, C: cbor::HasProtocolVersion> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row { status: d.decode_with(ctx)?, valid_until: d.decode_with(ctx)? })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use amaru_kernel::{any_constitutional_committee_member_status, prop_cbor_roundtrip};
    use proptest::{option, prelude::*, prop_compose};

    use super::*;

    prop_compose! {
        pub fn any_row()(
            status in option::of(any_constitutional_committee_member_status()),
            valid_until in option::of(any::<u64>()),
        ) -> Row {
            Row {
                status,
                valid_until: valid_until.map(Epoch::from),
            }
        }
    }

    prop_cbor_roundtrip!(Row, any_row());
}
