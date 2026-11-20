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

use amaru_kernel::{Epoch, EraHistory, GlobalParameters, NetworkName, ProtocolParameters};
use amaru_observability::info_span;

use crate::{
    StakeSummary,
    state::{
        StateError,
        tasks::background_task::{Cancellable, Runnable},
    },
    store::Snapshot,
    summary::{governance::GovernanceSummary, rewards::RewardsSummary},
};

/// Compute the rewards for a given epoch `e` using a snapshot of the ledger at `e - 3` and
/// the previous snapshot at `e - 1`.
pub struct ComputeRewards<PreviousSnapshot, RewardsSnapshot> {
    epoch: Epoch,
    /// Snapshot of the previous epoch; source of the rotated stake distribution as well as the
    /// block issuers and pots for the rewards calculation.
    previous_epoch_snapshot: PreviousSnapshot,
    /// Snapshot the rewards stake distribution is computed from, three epochs back.
    rewards_snapshot: RewardsSnapshot,
    network: NetworkName,
    global_parameters: GlobalParameters,
    protocol_parameters: ProtocolParameters,
    era_history: EraHistory,
}

impl<PreviousSnapshot, RewardsSnapshot> ComputeRewards<PreviousSnapshot, RewardsSnapshot> {
    pub fn new(
        epoch: Epoch,
        previous_epoch_snapshot: PreviousSnapshot,
        rewards_snapshot: RewardsSnapshot,
        network: NetworkName,
        global_parameters: GlobalParameters,
        protocol_parameters: ProtocolParameters,
        era_history: EraHistory,
    ) -> Self {
        Self {
            epoch,
            previous_epoch_snapshot,
            rewards_snapshot,
            network,
            global_parameters,
            protocol_parameters,
            era_history,
        }
    }
}

impl<PreviousSnapshot: Snapshot, RewardsSnapshot: Snapshot> Runnable<RewardsSummary>
    for ComputeRewards<PreviousSnapshot, RewardsSnapshot>
{
    /// Compute rewards for a given epoch using an anterior stake distribution.
    fn run(self, cancellable: Cancellable) -> Result<RewardsSummary, StateError> {
        cancellable.check_cancelled()?;
        info_span!(
            ledger::rewards::COMPUTE,
            for_epoch = self.epoch,
            using_stake_distribution_from_epoch = self.rewards_snapshot.epoch()
        )
        .in_scope(|| {
            let stake_summary = StakeSummary::new(
                &self.rewards_snapshot,
                GovernanceSummary::new(&self.rewards_snapshot, &self.era_history)?,
                self.network,
                |_| {},
            )
            .map_err(StateError::Storage)?;

            cancellable.check_cancelled()?;
            Ok(RewardsSummary::new(
                stake_summary,
                &self.global_parameters,
                &self.protocol_parameters,
                self.previous_epoch_snapshot
                    .iter_block_issuers()
                    .map_err(StateError::Storage)?
                    .map(|(_, block)| block.slot_leader),
                self.previous_epoch_snapshot.pots()?,
            ))
        })
    }
}
