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

use std::collections::BTreeSet;

use amaru_kernel::{Epoch, EraHistory, ProtocolParameters, StakeCredential};
use amaru_observability::info_span;

use crate::{
    governance::ratification::RatificationContext,
    state::{StateError, volatile::VolatileView},
    store::{ReadStore, StoreError},
};

mod stable_deregistrations;
pub use stable_deregistrations::StableDeregistrations;

mod volatile_registrations;
pub use volatile_registrations::{VolatileRegistrationStatus, VolatileRegistrations};

mod pools_updates;
pub use pools_updates::PoolsEpochTransitionUpdates;

mod rewards_state;
pub use rewards_state::{Computed, Effective, Rewards, RewardsState};

mod ratification;
pub use ratification::{GovernanceActivity, GovernanceUpdates};

/// Ends the ongoing epoch by calculating rewards payouts to the various still-registered accounts.
/// Unpaid rewards are assigned back to the treasury.
///
pub fn end_epoch<'volatile, 'store, DB: ReadStore>(
    view: &mut VolatileView<'volatile, 'store, DB>,
    stable_deregistrations: &StableDeregistrations,
    computed_rewards: Rewards<Computed>,
    next_epoch: Epoch,
) -> Result<Rewards<Effective>, StoreError> {
    info_span!(ledger::epoch_transition::END_EPOCH).in_scope(|| {
        // The reward window opens at `next_epoch - 4` (snapshot epoch = (next_epoch - 1) - 3).
        // Until the stable_deregistrations has covered it, we fall back to a full accounts db scan in
        // order to find out which accounts are unclaimed to get effective rewards.
        if !stable_deregistrations.covers(next_epoch - 4) {
            return Ok(Rewards::<Effective>::new(computed_rewards, view.iter_accounts()?));
        }

        // Otherwise, get the member registrations / deregistrations that happened in the volatile view.
        let volatile_registrations = view.volatile_registrations();

        let leaders = computed_rewards.leader_accounts();
        let mut unclaimed: BTreeSet<StakeCredential> = BTreeSet::new();

        // Members still unregistered at the end of the volatile window are unclaimed accounts
        for account in volatile_registrations.unregistered() {
            if !leaders.contains(account) && computed_rewards.has_reward(account) {
                unclaimed.insert(account.clone());
            }
        }

        // Members unregistered in stable blocks, and not since re-registered are also
        // unclaimed accounts
        for account in stable_deregistrations.unregistered_accounts() {
            if !volatile_registrations.is_registered(account)
                && !leaders.contains(account)
                && computed_rewards.has_reward(account)
            {
                unclaimed.insert(account.clone());
            }
        }

        // Leaders accounts are unclaimed if they have been deregistered in the volatile view.
        // If no change happened in the volatile view, we check the stable db to see if the account is
        // still there. This is a disk read but only on leader accounts which is a smaller subset of accounts
        // than member accounts.
        for leader in leaders.iter() {
            let unregistered = match volatile_registrations.latest_registration(leader) {
                VolatileRegistrationStatus::Registered => false,
                VolatileRegistrationStatus::Unregistered => true,
                VolatileRegistrationStatus::Unknown => !view.account_exists(leader)?,
            };

            if unregistered {
                unclaimed.insert(leader.clone());
            }
        }

        Ok(Rewards::<Effective>::from_unclaimed(computed_rewards, unclaimed))
    })
}

pub fn begin_epoch<'distr, 'volatile, 'store, DB: ReadStore>(
    view: &mut VolatileView<'volatile, 'store, DB>,
    epoch: Epoch,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    ratification_context: RatificationContext<'distr>,
) -> Result<(PoolsEpochTransitionUpdates, GovernanceUpdates), StateError> {
    info_span!(ledger::epoch_transition::BEGIN_EPOCH).in_scope(|| {
        // Compute the updates to perform on pools at the epoch boundary. This uses information
        // from both the immutable store and the volatile database, since we compute the updates
        // before they are "stable" and safe to store.
        let pools_updates = PoolsEpochTransitionUpdates::new(view.iter_pools()?, epoch);

        // Ratify and enact proposals at the epoch boundary. Note that this does not modify the
        // immutable store in any fashion (db is read-only here) but produces a series of
        // governance updates to be applied to the database once stable; and use in-memory in the
        // meantime.
        let governance_updates = GovernanceUpdates::new(
            view.proposals_roots()?,
            view.iter_proposals()?,
            era_history,
            protocol_parameters,
            ratification_context,
        )?;

        // FIXME: unbind accounts of unregistered pools
        //
        // We also need a mechanism to remove any remaining delegation to pools retired at the
        // epoch boundary.
        //
        // The accounts are already filtered out when computing rewards, but if any retired pool
        // were to re-register, they would automatically be granted the stake associated to their
        // past delegates.

        Ok((pools_updates, governance_updates))
    })
}
