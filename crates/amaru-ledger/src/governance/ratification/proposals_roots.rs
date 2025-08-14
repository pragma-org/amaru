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

use crate::governance::ratification::ProposalEnum;
use amaru_kernel::{cbor, ComparableProposalId};

#[derive(Debug, Default)]
pub struct ProposalsRoots {
    pub protocol_parameters: Option<ComparableProposalId>,
    pub hard_fork: Option<ComparableProposalId>,
    pub constitutional_committee: Option<ComparableProposalId>,
    pub constitution: Option<ComparableProposalId>,
}

impl ProposalsRoots {
    pub fn matching(&self, proposal: &ProposalEnum) -> bool {
        match proposal {
            // Orphans have no parents, so no roots. Hence it always _matches_.
            ProposalEnum::Orphan(..) => true,

            ProposalEnum::ProtocolParameters(_, parent) => {
                parent.as_deref() == self.protocol_parameters.as_ref()
            }

            ProposalEnum::HardFork(_, parent) => parent.as_deref() == self.hard_fork.as_ref(),

            ProposalEnum::ConstitutionalCommittee(_, parent) => {
                parent.as_deref() == self.constitutional_committee.as_ref()
            }

            ProposalEnum::Constitution(_, parent) => {
                parent.as_deref() == self.constitution.as_ref()
            }
        }
    }
}

impl<C> cbor::encode::Encode<C> for ProposalsRoots {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_map()?;
        e.u8(0)?;
        e.encode_with(&self.protocol_parameters, ctx)?;
        e.u8(1)?;
        e.encode_with(&self.hard_fork, ctx)?;
        e.u8(2)?;
        e.encode_with(&self.constitutional_committee, ctx)?;
        e.u8(3)?;
        e.encode_with(&self.constitution, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for ProposalsRoots {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
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
