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

use amaru_kernel::{cbor, ComparableProposalId};
use std::rc::Rc;

pub type ProposalsRoots = GenericProposalsRoots<ComparableProposalId>;

pub type ProposalsRootsRc = GenericProposalsRoots<Rc<ComparableProposalId>>;

#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct GenericProposalsRoots<T> {
    pub protocol_parameters: Option<T>,
    pub hard_fork: Option<T>,
    pub constitutional_committee: Option<T>,
    pub constitution: Option<T>,
}

impl<T> From<GenericProposalsRoots<T>> for GenericProposalsRoots<Rc<T>> {
    fn from(roots: GenericProposalsRoots<T>) -> Self {
        Self {
            protocol_parameters: roots.protocol_parameters.map(Rc::new),
            hard_fork: roots.hard_fork.map(Rc::new),
            constitutional_committee: roots.constitutional_committee.map(Rc::new),
            constitution: roots.constitution.map(Rc::new),
        }
    }
}
impl<C, T: AsRef<ComparableProposalId>> cbor::encode::Encode<C> for GenericProposalsRoots<T> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_map()?;
        e.u8(0)?;
        e.encode_with(self.protocol_parameters.as_ref().map(AsRef::as_ref), ctx)?;
        e.u8(1)?;
        e.encode_with(self.hard_fork.as_ref().map(AsRef::as_ref), ctx)?;
        e.u8(2)?;
        e.encode_with(
            self.constitutional_committee.as_ref().map(AsRef::as_ref),
            ctx,
        )?;
        e.u8(3)?;
        e.encode_with(self.constitution.as_ref().map(AsRef::as_ref), ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for GenericProposalsRoots<ComparableProposalId> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // NOTE: This type is only ever created by *us*, so it is okay-ish to not check for the map
        // keys values and expect keys in the right order.
        d.map()?;
        d.u8()?;
        let protocol_parameters = d.decode_with(ctx)?;
        d.u8()?;
        let hard_fork = d.decode_with(ctx)?;
        d.u8()?;
        let constitutional_committee = d.decode_with(ctx)?;
        d.u8()?;
        let constitution = d.decode_with(ctx)?;
        d.skip()?;
        Ok(Self {
            protocol_parameters,
            hard_fork,
            constitutional_committee,
            constitution,
        })
    }
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::ProposalsRoots;
    use amaru_kernel::{prop_cbor_roundtrip, tests::any_comparable_proposal_id};
    use proptest::{option, prop_compose};

    prop_cbor_roundtrip!(ProposalsRoots, any_proposals_roots());

    prop_compose! {
        pub fn any_proposals_roots()(
            protocol_parameters in option::of(any_comparable_proposal_id()),
            hard_fork in option::of(any_comparable_proposal_id()),
            constitutional_committee in option::of(any_comparable_proposal_id()),
            constitution in option::of(any_comparable_proposal_id()),
        ) -> ProposalsRoots  {
            ProposalsRoots {
                protocol_parameters,
                hard_fork,
                constitutional_committee,
                constitution,
            }
        }
    }
}
