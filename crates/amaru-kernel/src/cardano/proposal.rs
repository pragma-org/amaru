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

use crate::{Anchor, GovernanceAction, Lovelace, ProposalId, RewardAccount, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Proposal {
    pub deposit: Lovelace,
    pub reward_account: RewardAccount,
    pub gov_action: GovernanceAction,
    pub anchor: Anchor,
}

impl Proposal {
    pub fn parent(&self) -> Option<&ProposalId> {
        use GovernanceAction::*;
        match &self.gov_action {
            ParameterChange(parent, _, _)
            | HardForkInitiation(parent, _)
            | NoConfidence(parent)
            | UpdateCommittee(parent, _, _, _)
            | NewConstitution(parent, _) => parent.as_ref(),
            TreasuryWithdrawals(..) | Information => None,
        }
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Proposal {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(4)?;
            Ok(Self {
                deposit: d.decode_with(ctx)?,
                reward_account: d.decode_with(ctx)?,
                gov_action: d.decode_with(ctx)?,
                anchor: d.decode_with(ctx)?,
            })
        })
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for Proposal {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?;

        e.encode_with(self.deposit, ctx)?;
        e.encode_with(&self.reward_account, ctx)?;
        e.encode_with(&self.gov_action, ctx)?;
        e.encode_with(&self.anchor, ctx)?;

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{prelude::*, prop_compose};

    use crate::{Lovelace, Proposal, any_anchor, any_gov_action, any_reward_account};

    prop_compose! {
        pub fn any_proposal()(
            deposit in any::<Lovelace>(),
            reward_account in any_reward_account(),
            gov_action in any_gov_action(),
            anchor in any_anchor(),
        ) -> Proposal {
            Proposal {
                deposit,
                reward_account,
                gov_action,
                anchor,
            }
        }
    }
}
