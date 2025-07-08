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

use amaru_kernel::{cbor, Ballot, Voter};
use iter_borrow::IterBorrow;

/// Iterator used to browse rows from the votes column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Value>>;

pub type Key = Voter;

pub type Value = Ballot;

#[derive(Debug, PartialEq)]
pub struct Row {
    voter: Voter,
    ballot: Ballot,
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?;
        e.encode_with(&self.voter, ctx)?;
        e.encode_with(&self.ballot.proposal, ctx)?;
        e.encode_with(&self.ballot.vote, ctx)?;
        e.encode_with(&self.ballot.anchor, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            voter: d.decode_with(ctx)?,
            ballot: Ballot {
                proposal: d.decode_with(ctx)?,
                vote: d.decode_with(ctx)?,
                anchor: d.decode_with(ctx)?,
            },
        })
    }
}

// #[cfg(test)]
// pub(crate) mod tests {
//     use super::*;
//     use crate::store::{
//         proposals::tests::
//         accounts::test::any_stake_credential,
//     };
//     use amaru_kernel::{
//         new_stake_address, prop_cbor_roundtrip, Bytes, Constitution, CostModel, CostModels,
//         DRepVotingThresholds, ExUnitPrices, ExUnits, GovAction, Hash, KeyValuePairs, Lovelace,
//         Network, Nullable, PoolVotingThresholds, ProposalId, ProtocolParamUpdate, ProtocolVersion,
//         RewardAccount, ScriptHash, Set, StakeCredential, StakePayload, UnitInterval,
//     };
//     use proptest::{option, prelude::*};
//
//     #[cfg(not(target_os = "windows"))]
//     prop_cbor_roundtrip!(prop_cbor_roundtrip_row, Row, any_row());
//
//     prop_cbor_roundtrip!(prop_cbor_roundtrip_key, Key, any_proposal_id());
//
//
// }
