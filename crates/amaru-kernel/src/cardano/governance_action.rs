// Copyright 2026 PRAGMA
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
use proptest::{
    collection,
    prelude::{Arbitrary, BoxedStrategy, Just, Strategy, any, prop_oneof},
};

use crate::{
    Constitution, Credential, Epoch, Hash, KeyValuePairs, Lovelace, ProposalId, ProtocolParamUpdate, ProtocolVersion,
    RationalNumber, RewardAccount, cbor, hash, utils::cbor::SerialisedAsSet,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GovernanceAction {
    ParameterChange(Option<ProposalId>, Box<ProtocolParamUpdate>, Option<Hash<{ hash::size::SCRIPT }>>),
    HardForkInitiation(Option<ProposalId>, ProtocolVersion),
    TreasuryWithdrawals(KeyValuePairs<RewardAccount, Lovelace>, Option<Hash<{ hash::size::SCRIPT }>>),
    NoConfidence(Option<ProposalId>),
    // TODO: align types with ConstitutionalCommitteeUpdate
    UpdateCommittee(Option<ProposalId>, Vec<Credential>, KeyValuePairs<Credential, Epoch>, RationalNumber),
    NewConstitution(Option<ProposalId>, Constitution),
    Information,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for GovernanceAction {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // NOTE: the array length is not asserted here; see the equivalent note on `Certificate`.
        cbor::heterogeneous_array(d, |d, _assert_len| {
            let variant = d.u16()?;

            match variant {
                0 => {
                    let a = d.decode_with(ctx)?;
                    let b = d.decode_with(ctx)?;
                    let c = d.decode_with(ctx)?;
                    Ok(Self::ParameterChange(a, b, c))
                }

                1 => {
                    let a = d.decode_with(ctx)?;
                    let b = d.decode_with(ctx)?;
                    Ok(Self::HardForkInitiation(a, b))
                }

                2 => {
                    let a = d.decode_with(ctx)?;
                    let b = d.decode_with(ctx)?;
                    Ok(Self::TreasuryWithdrawals(a, b))
                }

                3 => {
                    let a = d.decode_with(ctx)?;
                    Ok(Self::NoConfidence(a))
                }

                4 => {
                    let a = d.decode_with(ctx)?;
                    let SerialisedAsSet(b) = d.decode_with(ctx)?;
                    let c = d.decode_with(ctx)?;
                    let d = d.decode_with(ctx)?;
                    Ok(Self::UpdateCommittee(a, b, c, d))
                }

                5 => {
                    let a = d.decode_with(ctx)?;
                    let b = d.decode_with(ctx)?;
                    Ok(Self::NewConstitution(a, b))
                }

                6 => Ok(Self::Information),
                _ => Err(cbor::decode::Error::message("unknown variant id for governance action")),
            }
        })
    }
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for GovernanceAction {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::ParameterChange(a, b, c) => {
                e.array(4)?;
                e.u16(0)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
            }

            Self::HardForkInitiation(a, b) => {
                e.array(3)?;
                e.u16(1)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::TreasuryWithdrawals(a, b) => {
                e.array(3)?;
                e.u16(2)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::NoConfidence(a) => {
                e.array(2)?;
                e.u16(3)?;
                e.encode_with(a, ctx)?;
            }

            Self::UpdateCommittee(a, b, c, d) => {
                e.array(5)?;
                e.u16(4)?;
                e.encode_with(a, ctx)?;
                e.encode_with(SerialisedAsSet(b), ctx)?;
                e.encode_with(c, ctx)?;
                e.encode_with(d, ctx)?;
            }

            Self::NewConstitution(a, b) => {
                e.array(3)?;
                e.u16(5)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            // FIXME(cbor): CDDL says just "6", not group/array "(6)"?
            Self::Information => {
                e.array(1)?;
                e.u16(6)?;
            }
        }

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for GovernanceAction {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    #[allow(clippy::unwrap_used)]
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        let parameter_change =
            (any::<Option<ProposalId>>(), any::<ProtocolParamUpdate>(), any::<Option<Hash<{ hash::size::SCRIPT }>>>())
                .prop_map(|(parent, update, guardrails)| {
                    GovernanceAction::ParameterChange(parent, Box::new(update), guardrails)
                });

        let hard_fork_initiation = (any::<Option<ProposalId>>(), any::<ProtocolVersion>())
            .prop_map(|(parent, version)| GovernanceAction::HardForkInitiation(parent, version));

        let treasury_withdrawals =
            (collection::vec(any::<(RewardAccount, Lovelace)>(), 0..3), any::<Option<Hash<{ hash::size::SCRIPT }>>>())
                .prop_map(|(withdrawals, guardrails)| {
                    GovernanceAction::TreasuryWithdrawals(KeyValuePairs::try_from(withdrawals).unwrap(), guardrails)
                });

        let no_confidence = any::<Option<ProposalId>>().prop_map(GovernanceAction::NoConfidence);

        let update_committee = (
            any::<Option<ProposalId>>(),
            collection::btree_set(any::<Credential>(), 0..3),
            collection::vec(any::<(Credential, Epoch)>(), 0..3),
            any::<RationalNumber>(),
        )
            .prop_map(|(parent, to_remove, to_add, quorum)| {
                GovernanceAction::UpdateCommittee(
                    parent,
                    to_remove.into_iter().collect(),
                    KeyValuePairs::try_from(to_add).unwrap(),
                    quorum,
                )
            });

        let new_constitution = (any::<Option<ProposalId>>(), any::<Constitution>())
            .prop_map(|(parent, constitution)| GovernanceAction::NewConstitution(parent, constitution));

        prop_oneof![
            parameter_change,
            hard_fork_initiation,
            treasury_withdrawals,
            no_confidence,
            update_committee,
            new_constitution,
            Just(GovernanceAction::Information),
        ]
        .boxed()
    }
}
