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

use amaru_kernel::{Epoch, EraHistory, ProtocolParameters};
use amaru_observability::info_span;

use crate::{
    governance::ratification::RatificationContext,
    state::{StateError, volatile::VolatileView},
    store::{ReadStore, StoreError},
};

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
    computed_rewards: Rewards<Computed>,
) -> Result<Rewards<Effective>, StoreError> {
    info_span!(ledger::epoch_transition::END_EPOCH)
        .in_scope(|| Ok(Rewards::<Effective>::new(computed_rewards, view.iter_accounts()?)))
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

        Ok((pools_updates, governance_updates))
    })
}
