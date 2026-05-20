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

use amaru_kernel::{Epoch, EraHistory};
use amaru_observability::{info_span, trace_span};

use crate::{
    governance::ratification::RatificationContext,
    state::StateError,
    store::{ReadStore, StoreError},
    summary::rewards::{RewardsPayouts, RewardsSummary},
};

mod pools_updates;
pub use pools_updates::PoolsEpochTransitionUpdates;

mod rewards_state;
pub use rewards_state::RewardsState;

mod ratification;
pub use ratification::{GovernanceActivity, GovernanceUpdates};

/// Ends the ongoing epoch by calculating rewards payouts to the various still-registered accounts.
/// Unpaid rewards are assigned back to the treasury.
///
pub fn end_epoch(db: &impl ReadStore, mut rewards_summary: RewardsSummary) -> Result<RewardsPayouts, StoreError> {
    trace_span!(INFO, amaru_observability::amaru::ledger::state::END_EPOCH).in_scope(|| {
        let mut rewards_payouts =
            RewardsPayouts::new(rewards_summary.delta_reserves(), rewards_summary.delta_treasury());

        // FIXME: account de-registrations from the volatile db
        //
        // The following code only looks at accounts from the stable store which is missing the last
        // `k` blocks of an epoch. One may unregister its account in that last unstable chunk; so
        // we must filter them out.

        // FIXME: retain unregistered accounts for epoch transition
        //
        // We have to prune accounts from that have been unregistered in this epoch and can no longer
        // receive rewards. The number of accounts doing so is usually limited compared to the total
        // number of accounts (~1.5M on Mainnet). So instead of iterating through all accounts to see
        // which have disappeared, we could simply remember which accounts have unregistered in the
        // epoch and prune them here rapidly.
        //
        // With interning of the account key, each account weights ~8 bytes; so even if all accounts
        // were to unregister in the epoch (end of Cardano?), that'd still be ~11MB of resident memory.
        // So very negligeable.
        for (account, _) in db.iter_accounts()? {
            if let Some(rewards) = rewards_summary.extract_rewards(&account)
                && rewards > 0
            {
                rewards_payouts.add_account(account, rewards);
            }
        }

        rewards_payouts.add_unclaimed_rewards(rewards_summary.unclaimed_rewards());

        Ok(rewards_payouts)
    })
}

pub fn begin_epoch<'distr>(
    db: &impl ReadStore,
    epoch: Epoch,
    era_history: &EraHistory,
    ratification_context: RatificationContext<'distr>,
) -> Result<(PoolsEpochTransitionUpdates, GovernanceUpdates), StateError> {
    info_span!(amaru_observability::amaru::ledger::state::BEGIN_EPOCH).in_scope(|| {
        // Tick pools to compute their new state at the epoch boundary. Notice
        // how we tick with the _current epoch_ however, but we take the snapshot before
        // the tick since the actions are only effective once the epoch is crossed.
        //
        // FIXME: unbind accounts of unregistered pools
        //
        // We also need a mechanism to remove any remaining delegation to pools retired by this
        // step. The accounts are already filtered out when computing rewards, but if any retired
        // pool were to re-register, they would automatically be granted the stake associated to
        // their past delegates.
        let pools_updates = info_span!(amaru_observability::amaru::ledger::state::TICK_POOL).in_scope(|| {
            let mut pools_updates = PoolsEpochTransitionUpdates::default();

            for (_pool_id, pool) in db.iter_pools()? {
                pools_updates.tick_pool(epoch, pool)
            }

            Ok::<_, StoreError>(pools_updates)
        })?;

        // Ratify and enact proposals at the epoch boundary. Note that this does not modify the
        // immutable store in any fashion (db is read-only here) but produces a series of
        // governance updates to be applied to the database once stable; and use in-memory in the
        // meantime.
        let governance_updates = GovernanceUpdates::new(db, era_history, ratification_context)?;

        Ok((pools_updates, governance_updates))
    })
}
