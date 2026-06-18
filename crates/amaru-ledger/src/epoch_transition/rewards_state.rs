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

use std::{collections::BTreeMap, marker::PhantomData};

use amaru_kernel::{Lovelace, StakeCredential};

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
#[derive(Debug, Default, Clone)]
pub enum RewardsState {
    /// No rewards computed yet, and no pending rewards to apply.
    #[default]
    NotReady,

    /// Rewards have been computed but we haven't crossed the epoch boundary _yet_, so they are
    /// pending until ready to be applied.
    Computed(Rewards<Computed>),

    /// The epoch boundary has just been crossed and we are less than k blocks in it; so we have to
    /// refer to the summary to resolve the correct balance for each account.
    Effective(Rewards<Effective>),
}

/// A type-level marker to carry certain state information alongside the 'Rewards' type.
pub trait KnownRewardState {
    type UnclaimedRewards;
}

#[derive(Debug, Clone)]
pub struct Computed;
impl KnownRewardState for Computed {
    type UnclaimedRewards = ();
}

#[derive(Debug, Clone)]
pub struct Effective;
impl KnownRewardState for Effective {
    type UnclaimedRewards = BTreeMap<StakeCredential, Lovelace>;
}

impl RewardsState {
    /// Consume computed rewards from the state, if available.
    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        match std::mem::replace(self, Self::NotReady) {
            Self::NotReady | Self::Effective(_) => None,
            Self::Computed(computed) => Some(computed),
        }
    }
}

/// A slim version of the rewards summary trimmed from other fields which are no longer necessary
/// to remember at this point.
///
/// It comes with a 'STEP' type parameter which we used to make apparent the transition between
/// computed and effective rewards that occur at the epoch boundary. It ensures that we don't
/// misuse computed rewards too early, and it reduces the amount of boilerplate in having to create
/// multiple types.
#[derive(Debug, Clone)]
pub struct Rewards<STEP: KnownRewardState> {
    /// A type-level marker for 'STEP'
    step: PhantomData<STEP>,

    /// Amount to be subtracted from the reserves
    delta_reserves: Lovelace,

    /// Amount to be paid to the treasury
    delta_treasury: Lovelace,

    /// Per-account rewards, determined from their relative stake and their delegatee.
    accounts: BTreeMap<StakeCredential, Lovelace>,

    /// Per-account unclaimed rewards; this tracks accounts that should have received
    /// rewards but were no longer registered at the epoch boundary. We do not simply keep the
    /// amount to allow rolling back accounts into the pool if needs be.
    unclaimed: STEP::UnclaimedRewards,
}

impl Rewards<Computed> {
    pub fn new(
        delta_reserves: Lovelace,
        delta_treasury: Lovelace,
        accounts: BTreeMap<StakeCredential, Lovelace>,
    ) -> Self {
        Self { delta_reserves, delta_treasury, accounts, unclaimed: (), step: PhantomData }
    }

    /// Fetch and remove from the summary rewards pertaining to a given account, if any.
    fn extract_rewards(&mut self, account: &StakeCredential) -> Option<Lovelace> {
        self.accounts.remove(account)
    }

    /// Return leftovers rewards that couldn't be allocated to account because they no longer
    /// exist. This method consumes (i.e. takes ownership) of the item because it is meant to be
    /// called last.
    fn unclaimed_rewards(&mut self) -> BTreeMap<StakeCredential, Lovelace> {
        std::mem::take(&mut self.accounts)
    }
}

/// Provides a mechanism for rolling effective rewards back to computed rewards by merging back the
/// unclaimed accounts into them.
impl From<Rewards<Effective>> for Rewards<Computed> {
    fn from(mut effective_rewards: Rewards<Effective>) -> Self {
        effective_rewards.accounts.append(&mut effective_rewards.unclaimed);
        Self {
            delta_reserves: effective_rewards.delta_reserves,
            delta_treasury: effective_rewards.delta_treasury,
            accounts: effective_rewards.accounts,
            unclaimed: (),
            step: PhantomData,
        }
    }
}

impl Rewards<Effective> {
    /// Compute the effective rewards from a current set of existing accounts.
    pub fn new(mut computed_rewards: Rewards<Computed>, accounts: impl Iterator<Item = StakeCredential>) -> Self {
        let mut effective_rewards: Rewards<Effective> = Self {
            delta_reserves: computed_rewards.delta_reserves,
            delta_treasury: computed_rewards.delta_treasury,
            accounts: BTreeMap::new(),
            unclaimed: BTreeMap::new(),
            step: PhantomData,
        };

        // TODO: retain unregistered accounts for epoch transition instead of searching for them
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
        for account in accounts {
            if let Some(rewards) = computed_rewards.extract_rewards(&account)
                && rewards > 0
            {
                effective_rewards.accounts.insert(account, rewards);
            }
        }

        effective_rewards.unclaimed = computed_rewards.unclaimed_rewards();
        effective_rewards.unclaimed.retain(|_, rewards| *rewards > 0);

        effective_rewards
    }

    pub fn accounts(&self) -> &BTreeMap<StakeCredential, Lovelace> {
        &self.accounts
    }

    /// Get rewards for a specific account, consuming the account.
    pub fn pop_account(&mut self, account: &StakeCredential) -> Lovelace {
        self.accounts.remove(account).unwrap_or(0)
    }

    /// Amount to be paid to the reserves
    pub fn delta_reserves(&self) -> Lovelace {
        self.delta_reserves
    }

    /// Amount to be paid to the treasury
    pub fn delta_treasury(&self) -> Lovelace {
        self.delta_treasury + self.unclaimed.values().sum::<Lovelace>()
    }
}
