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

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy, any};

use crate::{ProposalId, Voter, cbor};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BallotId {
    pub proposal: ProposalId,
    pub voter: Voter,
}

impl BallotId {
    /// Returns the CBOR prefix resulting from encoding this ballot with a given proposal id but an
    /// unknown voter.
    pub fn encode_prefix<W: cbor::encode::Write>(
        proposal: &ProposalId,
        e: &mut cbor::Encoder<W>,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode(proposal)?;
        Ok(())
    }
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for BallotId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.proposal, ctx)?;
        e.encode_with(self.voter, ctx)?;
        Ok(())
    }
}

impl<'d, C: cbor::HasProtocolVersion> cbor::decode::Decode<'d, C> for BallotId {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self { proposal: d.decode_with(ctx)?, voter: d.decode_with(ctx)? })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for BallotId {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (any::<ProposalId>(), any::<Voter>()).prop_map(|(proposal, voter)| BallotId { proposal, voter }).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::BallotId;
    use crate::prop_cbor_roundtrip;

    prop_cbor_roundtrip!(BallotId);
}
