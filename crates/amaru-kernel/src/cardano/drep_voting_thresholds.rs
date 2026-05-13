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

pub use pallas_primitives::conway::DRepVotingThresholds;
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
