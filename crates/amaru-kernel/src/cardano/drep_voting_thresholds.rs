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

use std::fmt;

use crate::{RationalNumber, cbor, rational_number};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DRepVotingThresholds {
    pub motion_no_confidence: RationalNumber,
    pub committee_normal: RationalNumber,
    pub committee_no_confidence: RationalNumber,
    pub update_constitution: RationalNumber,
    pub hard_fork_initiation: RationalNumber,
    pub pp_network_group: RationalNumber,
    pub pp_economic_group: RationalNumber,
    pub pp_technical_group: RationalNumber,
    pub pp_governance_group: RationalNumber,
    pub treasury_withdrawal: RationalNumber,
}

impl<'b, C> cbor::Decode<'b, C> for DRepVotingThresholds {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;

        Ok(Self {
            motion_no_confidence: d.decode_with(ctx)?,
            committee_normal: d.decode_with(ctx)?,
            committee_no_confidence: d.decode_with(ctx)?,
            update_constitution: d.decode_with(ctx)?,
            hard_fork_initiation: d.decode_with(ctx)?,
            pp_network_group: d.decode_with(ctx)?,
            pp_economic_group: d.decode_with(ctx)?,
            pp_technical_group: d.decode_with(ctx)?,
            pp_governance_group: d.decode_with(ctx)?,
            treasury_withdrawal: d.decode_with(ctx)?,
        })
    }
}

impl<C> cbor::Encode<C> for DRepVotingThresholds {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(10)?;

        e.encode_with(&self.motion_no_confidence, ctx)?;
        e.encode_with(&self.committee_normal, ctx)?;
        e.encode_with(&self.committee_no_confidence, ctx)?;
        e.encode_with(&self.update_constitution, ctx)?;
        e.encode_with(&self.hard_fork_initiation, ctx)?;
        e.encode_with(&self.pp_network_group, ctx)?;
        e.encode_with(&self.pp_economic_group, ctx)?;
        e.encode_with(&self.pp_technical_group, ctx)?;
        e.encode_with(&self.pp_governance_group, ctx)?;
        e.encode_with(&self.treasury_withdrawal, ctx)?;

        Ok(())
    }
}

impl fmt::Display for DRepVotingThresholds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: destructuring for completeness static checks
        let DRepVotingThresholds {
            motion_no_confidence,
            committee_normal,
            committee_no_confidence,
            update_constitution,
            hard_fork_initiation,
            pp_network_group,
            pp_economic_group,
            pp_technical_group,
            pp_governance_group,
            treasury_withdrawal,
        } = self;

        write!(
            f,
            "{{\
            committee_normal={}, \
            committee_no_confidence={}, \
            motion_no_confidence={}, \
            treasury_withdrawal={}, \
            update_constitution={}, \
            pp_network_group={}, \
            pp_economic_group={}, \
            pp_technical_group={}, \
            pp_governance_group={}, \
            hard_fork_initiation={}\
        }}",
            rational_number::fmt(committee_normal),
            rational_number::fmt(committee_no_confidence),
            rational_number::fmt(motion_no_confidence),
            rational_number::fmt(treasury_withdrawal),
            rational_number::fmt(update_constitution),
            rational_number::fmt(pp_network_group),
            rational_number::fmt(pp_economic_group),
            rational_number::fmt(pp_technical_group),
            rational_number::fmt(pp_governance_group),
            rational_number::fmt(hard_fork_initiation),
        )
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use proxy::*;

#[cfg(any(test, feature = "test-utils"))]
mod proxy {
    use serde::Deserialize;

    use super::DRepVotingThresholds;
    use crate::{RationalNumber, utils::serde::HasProxy};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CommitteeProxy {
        default: RationalNumber,
        state_of_no_confidence: RationalNumber,
    }

    #[derive(Deserialize)]
    struct PpuProxy {
        network: RationalNumber,
        economic: RationalNumber,
        technical: RationalNumber,
        governance: RationalNumber,
    }

    /// Fixture JSON shape with the no-confidence/committee/hard-fork/PPU fields regrouped.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DRepVotingThresholdsProxy {
        no_confidence: RationalNumber,
        constitution: RationalNumber,
        constitutional_committee: CommitteeProxy,
        hard_fork_initiation: RationalNumber,
        protocol_parameters_update: PpuProxy,
        treasury_withdrawals: RationalNumber,
    }

    impl From<DRepVotingThresholdsProxy> for DRepVotingThresholds {
        fn from(p: DRepVotingThresholdsProxy) -> Self {
            DRepVotingThresholds {
                motion_no_confidence: p.no_confidence,
                committee_normal: p.constitutional_committee.default,
                committee_no_confidence: p.constitutional_committee.state_of_no_confidence,
                update_constitution: p.constitution,
                hard_fork_initiation: p.hard_fork_initiation,
                pp_network_group: p.protocol_parameters_update.network,
                pp_economic_group: p.protocol_parameters_update.economic,
                pp_technical_group: p.protocol_parameters_update.technical,
                pp_governance_group: p.protocol_parameters_update.governance,
                treasury_withdrawal: p.treasury_withdrawals,
            }
        }
    }

    impl HasProxy for DRepVotingThresholds {
        type Proxy = DRepVotingThresholdsProxy;
    }
}
