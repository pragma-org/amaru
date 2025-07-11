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

use crate::{cbor, Epoch, Proposal, ProposalId};

#[derive(Debug)]
pub struct ProposalState {
    pub id: ProposalId,
    pub procedure: Proposal,
    pub proposed_in: Epoch,
    #[allow(dead_code)]
    pub expires_after: Epoch,
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProposalState {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let id = d.decode_with(ctx)?;
        d.skip()?; // CC Votes
        d.skip()?; // DRep Votes
        d.skip()?; // SPO Votes
        let procedure = d.decode_with(ctx)?;
        let proposed_in = d.decode_with(ctx)?;
        let expires_after = d.decode_with(ctx)?;

        Ok(ProposalState {
            id,
            procedure,
            proposed_in,
            expires_after,
        })
    }
}
