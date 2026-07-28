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

use crate::utils::cbor::SerialisedAsSet;
pub use crate::{
    Anchor, DRep, Epoch, Hash, Lovelace, Nullable, PoolId, PoolMetadata, RationalNumber, Relay, RewardAccount, Set,
    StakeCredential, cbor,
    size::{KEY, VRF_KEY},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Certificate {
    StakeRegistration(StakeCredential),
    StakeDeregistration(StakeCredential),
    StakeDelegation(StakeCredential, PoolId),
    // TODO: reduce duplication with [`crate::PoolParams`]
    PoolRegistration {
        operator: PoolId,
        vrf_keyhash: Hash<VRF_KEY>,
        pledge: Lovelace,
        cost: Lovelace,
        margin: RationalNumber,
        reward_account: RewardAccount,
        pool_owners: Vec<Hash<KEY>>,
        relays: Vec<Relay>,
        pool_metadata: Nullable<PoolMetadata>,
    },
    PoolRetirement(PoolId, Epoch),
    Reg(StakeCredential, Lovelace),
    UnReg(StakeCredential, Lovelace),
    VoteDeleg(StakeCredential, DRep),
    StakeVoteDeleg(StakeCredential, PoolId, DRep),
    StakeRegDeleg(StakeCredential, PoolId, Lovelace),
    VoteRegDeleg(StakeCredential, DRep, Lovelace),
    StakeVoteRegDeleg(StakeCredential, PoolId, DRep, Lovelace),
    AuthCommitteeHot(StakeCredential, StakeCredential),
    ResignCommitteeCold(StakeCredential, Nullable<Anchor>),
    RegDRepCert(StakeCredential, Lovelace, Nullable<Anchor>),
    UnRegDRepCert(StakeCredential, Lovelace),
    UpdateDRepCert(StakeCredential, Nullable<Anchor>),
}

impl<C> cbor::encode::Encode<C> for Certificate {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::StakeRegistration(a) => {
                e.array(2)?;
                e.u16(0)?;
                e.encode_with(a, ctx)?;
            }

            Self::StakeDeregistration(a) => {
                e.array(2)?;
                e.u16(1)?;
                e.encode_with(a, ctx)?;
            }

            Self::StakeDelegation(a, b) => {
                e.array(3)?;
                e.u16(2)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::PoolRegistration {
                operator,
                vrf_keyhash,
                pledge,
                cost,
                margin,
                reward_account,
                pool_owners,
                relays,
                pool_metadata,
            } => {
                e.array(10)?;
                e.u16(3)?;

                e.encode_with(operator, ctx)?;
                e.encode_with(vrf_keyhash, ctx)?;
                e.encode_with(pledge, ctx)?;
                e.encode_with(cost, ctx)?;
                e.encode_with(margin, ctx)?;
                e.encode_with(reward_account, ctx)?;
                e.encode_with(SerialisedAsSet(pool_owners), ctx)?;
                e.encode_with(relays, ctx)?;
                e.encode_with(pool_metadata, ctx)?;
            }

            Self::PoolRetirement(a, b) => {
                e.array(3)?;
                e.u16(4)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            // 5 and 6 removed since the Conway era
            Self::Reg(a, b) => {
                e.array(3)?;
                e.u16(7)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::UnReg(a, b) => {
                e.array(3)?;
                e.u16(8)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::VoteDeleg(a, b) => {
                e.array(3)?;
                e.u16(9)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::StakeVoteDeleg(a, b, c) => {
                e.array(4)?;
                e.u16(10)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
            }

            Self::StakeRegDeleg(a, b, c) => {
                e.array(4)?;
                e.u16(11)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
            }

            Self::VoteRegDeleg(a, b, c) => {
                e.array(4)?;
                e.u16(12)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
            }

            Self::StakeVoteRegDeleg(a, b, c, d) => {
                e.array(5)?;
                e.u16(13)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
                e.encode_with(d, ctx)?;
            }

            Self::AuthCommitteeHot(a, b) => {
                e.array(3)?;
                e.u16(14)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::ResignCommitteeCold(a, b) => {
                e.array(3)?;
                e.u16(15)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::RegDRepCert(a, b, c) => {
                e.array(4)?;
                e.u16(16)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
            }

            Self::UnRegDRepCert(a, b) => {
                e.array(3)?;
                e.u16(17)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }

            Self::UpdateDRepCert(a, b) => {
                e.array(3)?;
                e.u16(18)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }
        }

        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Certificate {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let variant = d.u16()?;

        match variant {
            0 => {
                let a = d.decode_with(ctx)?;
                Ok(Self::StakeRegistration(a))
            }

            1 => {
                let a = d.decode_with(ctx)?;
                Ok(Self::StakeDeregistration(a))
            }

            2 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::StakeDelegation(a, b))
            }

            3 => {
                let operator = d.decode_with(ctx)?;
                let vrf_keyhash = d.decode_with(ctx)?;
                let pledge = d.decode_with(ctx)?;
                let cost = d.decode_with(ctx)?;
                let margin = d.decode_with(ctx)?;
                let reward_account = d.decode_with(ctx)?;
                let SerialisedAsSet(pool_owners) = d.decode_with(ctx)?;
                let relays = d.decode_with(ctx)?;
                let pool_metadata = d.decode_with(ctx)?;

                Ok(Self::PoolRegistration {
                    operator,
                    vrf_keyhash,
                    pledge,
                    cost,
                    margin,
                    reward_account,
                    pool_owners,
                    relays,
                    pool_metadata,
                })
            }

            4 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::PoolRetirement(a, b))
            }

            // 5 and 6 removed since the Conway era
            7 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::Reg(a, b))
            }

            8 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::UnReg(a, b))
            }

            9 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::VoteDeleg(a, b))
            }

            10 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                let c = d.decode_with(ctx)?;
                Ok(Self::StakeVoteDeleg(a, b, c))
            }

            11 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                let c = d.decode_with(ctx)?;
                Ok(Self::StakeRegDeleg(a, b, c))
            }

            12 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                let c = d.decode_with(ctx)?;
                Ok(Self::VoteRegDeleg(a, b, c))
            }

            13 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                let c = d.decode_with(ctx)?;
                let d = d.decode_with(ctx)?;
                Ok(Self::StakeVoteRegDeleg(a, b, c, d))
            }

            14 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::AuthCommitteeHot(a, b))
            }

            15 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::ResignCommitteeCold(a, b))
            }

            16 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                let c = d.decode_with(ctx)?;
                Ok(Self::RegDRepCert(a, b, c))
            }

            17 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::UnRegDRepCert(a, b))
            }

            18 => {
                let a = d.decode_with(ctx)?;
                let b = d.decode_with(ctx)?;
                Ok(Self::UpdateDRepCert(a, b))
            }

            _ => Err(cbor::decode::Error::message("unknown variant id for certificate")),
        }
    }
}
