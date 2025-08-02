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

use crate::{cbor, decode_array, Anchor, ProposalId, Vote};

#[derive(Debug, PartialEq)]
pub struct Ballot {
    pub proposal: ProposalId,
    pub vote: Vote,
    pub anchor: Option<Anchor>,
}

impl<C> cbor::encode::Encode<C> for Ballot {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.encode_with(&self.proposal, ctx)?;
        e.encode_with(&self.vote, ctx)?;
        e.encode_with(&self.anchor, ctx)?;
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for Ballot {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        decode_array(d, 3, |d| {
            Ok(Ballot {
                proposal: d.decode_with(ctx)?,
                vote: d.decode_with(ctx)?,
                anchor: d.decode_with(ctx)?,
            })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::Ballot;
    use crate::{
        anchor::tests::any_anchor, prop_cbor_roundtrip, proposal_id::tests::any_proposal_id, Vote,
    };
    use proptest::{option, prelude::*, prop_compose};

    pub fn any_vote() -> impl Strategy<Value = Vote> {
        prop_oneof![Just(Vote::Yes), Just(Vote::No), Just(Vote::Abstain)]
    }

    prop_compose! {
        pub fn any_ballot()(
            proposal in any_proposal_id(),
            vote in any_vote(),
            anchor in option::of(any_anchor()),
        ) -> Ballot  {
            Ballot {
                proposal,
                vote,
                anchor,
            }
        }
    }

    prop_cbor_roundtrip!(Ballot, any_ballot());
}
