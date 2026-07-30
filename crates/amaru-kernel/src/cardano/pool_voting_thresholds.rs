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

use crate::{RationalNumber, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolVotingThresholds {
    pub motion_no_confidence: RationalNumber,
    pub committee_normal: RationalNumber,
    pub committee_no_confidence: RationalNumber,
    pub hard_fork_initiation: RationalNumber,
    pub security_voting_threshold: RationalNumber,
}

impl fmt::Display for PoolVotingThresholds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: destructuring for completeness static checks
        let PoolVotingThresholds {
            motion_no_confidence,
            committee_normal,
            committee_no_confidence,
            hard_fork_initiation,
            security_voting_threshold,
        } = self;

        write!(
            f,
            "{{ \
            committee_normal={committee_normal}, \
            committee_no_confidence={committee_no_confidence}, \
            motion_no_confidence={motion_no_confidence}, \
            hard_fork_initiation={hard_fork_initiation}, \
            security_voting_threshold={security_voting_threshold} \
            }}",
        )
    }
}

impl<'b, C> cbor::Decode<'b, C> for PoolVotingThresholds {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;

        Ok(Self {
            motion_no_confidence: d.decode_with(ctx)?,
            committee_normal: d.decode_with(ctx)?,
            committee_no_confidence: d.decode_with(ctx)?,
            hard_fork_initiation: d.decode_with(ctx)?,
            security_voting_threshold: d.decode_with(ctx)?,
        })
    }
}

impl<C> cbor::Encode<C> for PoolVotingThresholds {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(5)?;

        e.encode_with(self.motion_no_confidence, ctx)?;
        e.encode_with(self.committee_normal, ctx)?;
        e.encode_with(self.committee_no_confidence, ctx)?;
        e.encode_with(self.hard_fork_initiation, ctx)?;
        e.encode_with(self.security_voting_threshold, ctx)?;

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use proxy::*;

#[cfg(any(test, feature = "test-utils"))]
mod proxy {
    use serde::Deserialize;

    use super::PoolVotingThresholds;
    use crate::{RationalNumber, utils::serde::HasProxy};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CommitteeProxy {
        default: RationalNumber,
        state_of_no_confidence: RationalNumber,
    }

    #[derive(Deserialize)]
    struct PpuProxy {
        security: RationalNumber,
    }

    /// Fixture JSON shape with the no-confidence/committee/hard-fork/PPU fields regrouped.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PoolVotingThresholdsProxy {
        no_confidence: RationalNumber,
        constitutional_committee: CommitteeProxy,
        hard_fork_initiation: RationalNumber,
        protocol_parameters_update: PpuProxy,
    }

    impl From<PoolVotingThresholdsProxy> for PoolVotingThresholds {
        fn from(p: PoolVotingThresholdsProxy) -> Self {
            PoolVotingThresholds {
                motion_no_confidence: p.no_confidence,
                committee_normal: p.constitutional_committee.default,
                committee_no_confidence: p.constitutional_committee.state_of_no_confidence,
                hard_fork_initiation: p.hard_fork_initiation,
                security_voting_threshold: p.protocol_parameters_update.security,
            }
        }
    }

    impl HasProxy for PoolVotingThresholds {
        type Proxy = PoolVotingThresholdsProxy;
    }
}
