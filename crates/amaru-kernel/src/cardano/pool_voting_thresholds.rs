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
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(5)?;
            Ok(Self {
                motion_no_confidence: d.decode_with(ctx)?,
                committee_normal: d.decode_with(ctx)?,
                committee_no_confidence: d.decode_with(ctx)?,
                hard_fork_initiation: d.decode_with(ctx)?,
                security_voting_threshold: d.decode_with(ctx)?,
            })
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
