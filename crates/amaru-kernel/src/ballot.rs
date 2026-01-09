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

use crate::{Anchor, Vote, cbor, heterogeneous_array};

#[derive(Clone, Debug, PartialEq)]
pub struct Ballot {
    vote: Vote,

    /// NOTE: Anchors are kept in a Box since
    ///
    /// - Anchors are often absent from votes (~60% of the time on Mainnet); thus reducing the
    ///   memory footprint of this from 64B to 16B
    ///
    /// - Rarely (i.e. never) accessed anyway by the ledger which completely neglects the cost of
    ///   the indirection.
    ///
    /// One may ask: if never accessed, why keep it around?
    ///
    /// -> Because it is still on-chain data which may be relevant to users even though it has no
    ///    influence on the ledger validations.
    anchor: Option<Box<Anchor>>,
}

impl Ballot {
    pub fn new(vote: Vote, anchor: Option<Anchor>) -> Self {
        Self {
            vote,
            anchor: anchor.map(Box::new),
        }
    }

    pub fn vote(&self) -> &Vote {
        &self.vote
    }

    pub fn anchor(&self) -> Option<&Anchor> {
        self.anchor.as_deref()
    }
}

impl<C> cbor::encode::Encode<C> for Ballot {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.vote(), ctx)?;
        e.encode_with(self.anchor(), ctx)?;
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for Ballot {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self {
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
        prop_cbor_roundtrip,
        tests::{any_anchor, any_vote},
    };
    use proptest::{option, prelude::*};

    prop_compose! {
        pub fn any_ballot()(
            vote in any_vote(),
            anchor in option::of(any_anchor()),
        ) -> Ballot  {
            Ballot::new(vote, anchor)
        }
    }

    prop_cbor_roundtrip!(Ballot, any_ballot());
}
