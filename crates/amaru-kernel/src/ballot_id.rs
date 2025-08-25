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

use crate::{ComparableProposalId, Voter, cbor, heterogeneous_array};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BallotId {
    pub proposal: ComparableProposalId,
    pub voter: Voter,
}

impl<C> cbor::encode::Encode<C> for BallotId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(&self.proposal, ctx)?;
        e.encode_with(&self.voter, ctx)?;
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for BallotId {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self {
                proposal: d.decode_with(ctx)?,
                voter: d.decode_with(ctx)?,
            })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::BallotId;
    use crate::{Hash, Voter, prop_cbor_roundtrip, tests::any_comparable_proposal_id};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(BallotId, any_ballot_id());

    pub fn any_hash28() -> impl Strategy<Value = Hash<28>> {
        any::<[u8; 28]>().prop_map(Hash::from)
    }

    pub fn any_voter() -> impl Strategy<Value = Voter> {
        prop_oneof![
            any_hash28().prop_map(Voter::ConstitutionalCommitteeKey),
            any_hash28().prop_map(Voter::ConstitutionalCommitteeScript),
            any_hash28().prop_map(Voter::DRepKey),
            any_hash28().prop_map(Voter::DRepScript),
            any_hash28().prop_map(Voter::StakePoolKey),
        ]
    }

    prop_compose! {
        pub fn any_ballot_id()(
            proposal in any_comparable_proposal_id(),
            voter in any_voter(),
        ) -> BallotId {
            BallotId {
                proposal,
                voter,
            }
        }
    }
}
