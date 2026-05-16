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

use crate::summary::rewards::{RewardsPayouts, RewardsSummary};

/// Captures the lifecycle of rewards calculation throughout block applications. Rewards are
/// computed and later consumed/applied to accounts.
///
/// However, there's a period of time (precisely the last k blocks of an epoch) where rewards are
/// not yet persisted in the database, but they do count towards an account balance.
///
/// This is because we only modify the stable store once the information is immutable.
///
/// NOTE: thought-exercise: what would happen if we applied rewards immediately?
///
/// There are three scenarios:
///
/// 1. No chain switch occurs in first k blocks of the next epoch.
///    Rewards becomes indeed immutable. That's the happy path.
///
/// 2. A chain switch occurs but does not make us rollback beyond the epoch boundary.
///    That's also okay, because that means the previous rewards application is still valid. No big
///    deal.
///
/// 3. A chain switch occurs and causes a rollback that crosses the epoch boundary again.
///    Now that's bad, because the rollback becomes a lot more expensive; we have to go back
///    through each account and undo the rewards only to re-apply them again at the epoch
///    boundary.
///
///    One could say: why bother? Since we're going to re-apply the same rewards again (rewards
///    don't depend on the previous epoch, but the two before).
///
///    And the response is that it would impact the re-application of the rolled back blocks. For
///    example, an account could attempt to spend its rewards ahead of having received them! To
///    cope with that, we would have to remember the applied-but-rolled-back rewards but by that
///    time, we would have already consumed and thrown away the rewards summary. Plus, it opens the
///    door for subtle inconsistency bugs because our source of truth (the immutable store) now
///    needs a patch for anyone consuming that piece of information.
///
/// Thus, we don't apply rewards immediately on epoch boundary, but we keep them around for k more
/// blocks and perform an extra lookup when assessing the balance of an account.
#[derive(Debug)]
pub enum RewardsState {
    /// No rewards computed yet, and no pending rewards to apply.
    NotReady,

    /// Rewards have been computed but we haven't crossed the epoch boundary _yet_, so they are
    /// pending until ready to be applied.
    Ready(RewardsSummary),

    /// The epoch boundary has just been crossed and we are less than k blocks in it; so we have to
    /// refer to the summary to resolve the correct balance for each account.
    Effective(RewardsPayouts),
}

impl RewardsState {
    /// Consume a rewards summary, if available.
    pub fn take(&mut self) -> Option<RewardsSummary> {
        match std::mem::replace(self, Self::NotReady) {
            Self::NotReady | Self::Effective(_) => None,
            Self::Ready(summary) => Some(summary),
        }
    }
}
