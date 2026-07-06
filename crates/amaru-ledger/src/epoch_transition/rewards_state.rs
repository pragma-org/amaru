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

use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    sync::Arc,
};

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
#[derive(Debug, Default)]
pub enum RewardsState {
    /// No rewards computed yet, and no pending rewards to apply.
    #[default]
    NotReady,

    /// Rewards have been computed but we haven't crossed the epoch boundary _yet_, so they are
    /// pending until ready to be applied.
    ///
    /// Held behind an `Arc` so that snapshotting the volatile overlay (e.g. when capturing a
    /// rollback recovery) doesn't require a deep copy of the per-account rewards map.
    /// The rewards are only ever read or replaced wholesale while in the overlay, so
    /// no copy-on-write mutation is needed.
    Computed(Arc<Rewards<Computed>>),

    /// The epoch boundary has just been crossed and we are less than k blocks in it; so we have to
    /// refer to the summary to resolve the correct balance for each account.
    Effective(Arc<Rewards<Effective>>),
}

/// A type-level marker to carry certain state information alongside the 'Rewards' type.
pub trait KnownRewardState {
    type UnclaimedRewards;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Computed;
impl KnownRewardState for Computed {
    type UnclaimedRewards = ();
}

#[derive(Debug, PartialEq, Eq)]
pub struct Effective;
impl KnownRewardState for Effective {
    /// The accounts whose rewards are unclaimed (they unregistered during the epoch). Only the keys
    /// are kept: the amounts live in the shared `accounts` map, so an `Effective` differs from a
    /// `Computed` by nothing more than this small set of markers.
    type UnclaimedRewards = BTreeSet<StakeCredential>;
}

impl RewardsState {
    /// Snapshot the rewards state for rollback recovery. The rewards are held behind an `Arc`, so
    /// this only bumps the reference count rather than deep-copying the per-account map.
    pub fn snapshot(&self) -> RewardsState {
        match self {
            RewardsState::NotReady => RewardsState::NotReady,
            RewardsState::Computed(rewards) => RewardsState::Computed(Arc::clone(rewards)),
            RewardsState::Effective(rewards) => RewardsState::Effective(Arc::clone(rewards)),
        }
    }

    /// Consume computed rewards from the state, if available.
    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        match std::mem::replace(self, Self::NotReady) {
            Self::NotReady | Self::Effective(_) => None,
            Self::Computed(computed) => Some(Arc::try_unwrap(computed).unwrap_or_else(|rewards| rewards.snapshot())),
        }
    }

    /// Rollback the rewards state when rolling back across an epoch
    pub fn rollback(self) -> Self {
        match self {
            st @ (RewardsState::NotReady | RewardsState::Computed(..)) => st,
            RewardsState::Effective(effective) => {
                let effective = Arc::try_unwrap(effective).unwrap_or_else(|rewards| rewards.snapshot());
                RewardsState::Computed(Arc::new(effective.to_computed()))
            }
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
#[derive(Debug, PartialEq, Eq)]
pub struct Rewards<STEP: KnownRewardState> {
    /// A type-level marker for 'STEP'
    step: PhantomData<STEP>,

    /// Amount to be subtracted from the reserves
    delta_reserves: Lovelace,

    /// Amount to be paid to the treasury
    delta_treasury: Lovelace,

    /// Per-account rewards, determined from their relative stake and their delegatee. This holds
    /// *every* account with a reward, whether or not it is still registered; the `Effective` step
    /// carves out the unclaimed ones through `unclaimed`. It is behind an `Arc` so that converting
    /// between `Effective` and `Computed` (and snapshotting the overlay for rollback recovery)
    /// shares this map rather than copying it.
    accounts: Arc<BTreeMap<StakeCredential, Lovelace>>,

    /// The accounts whose rewards are unclaimed because they were no longer registered at the epoch
    /// boundary. For `Effective` this is the set of such account keys (their amounts stay in
    /// `accounts` and are summed back into the treasury); for `Computed` it is `()`. Rolling an
    /// `Effective` back to a `Computed` is therefore just dropping this set, and the reverse is
    /// re-attaching it — neither touches `accounts`.
    unclaimed: STEP::UnclaimedRewards,
}

impl<STEP: KnownRewardState> Rewards<STEP>
where
    STEP::UnclaimedRewards: Clone,
{
    /// Snapshot of the rewards summary, used when snapshotting the overlay for rollback recovery.
    /// This is deliberately a named method rather than a `Clone` implementation, but it is cheap:
    /// the per-account map is shared through `Arc` (a reference-count bump), and only the small
    /// `unclaimed` marker set is copied.
    pub fn snapshot(&self) -> Rewards<STEP> {
        Rewards {
            step: PhantomData,
            delta_reserves: self.delta_reserves,
            delta_treasury: self.delta_treasury,
            accounts: Arc::clone(&self.accounts),
            unclaimed: self.unclaimed.clone(),
        }
    }
}

impl Rewards<Computed> {
    pub fn new(
        delta_reserves: Lovelace,
        delta_treasury: Lovelace,
        accounts: BTreeMap<StakeCredential, Lovelace>,
    ) -> Self {
        Self { delta_reserves, delta_treasury, accounts: Arc::new(accounts), unclaimed: (), step: PhantomData }
    }
}

impl Rewards<Effective> {
    /// Compute the effective rewards from a current set of existing accounts.
    ///
    /// The full per-account map is shared as-is; we only flag which accounts are *unclaimed*, i.e.
    /// have a reward but were no longer registered at the epoch boundary. Their amounts stay in the
    /// shared map (they are folded back into the treasury via [`Self::delta_treasury`]) but they are
    /// never paid out to the account.
    pub fn new(computed_rewards: Rewards<Computed>, accounts: impl Iterator<Item = StakeCredential>) -> Self {
        // The set of accounts still registered at the boundary, used to tell claimed from unclaimed.
        let registered: BTreeSet<StakeCredential> = accounts.collect();

        let unclaimed: BTreeSet<StakeCredential> = computed_rewards
            .accounts
            .iter()
            .filter(|(account, reward)| **reward > 0 && !registered.contains(*account))
            .map(|(account, _)| account.clone())
            .collect();

        Rewards {
            step: PhantomData,
            delta_reserves: computed_rewards.delta_reserves,
            delta_treasury: computed_rewards.delta_treasury,
            accounts: computed_rewards.accounts,
            unclaimed,
        }
    }

    /// The reward payable to a specific account: its computed reward if it is still registered, or
    /// `0` if it is unclaimed (in which case the amount goes to the treasury instead) or has none.
    pub fn reward_of(&self, account: &StakeCredential) -> Lovelace {
        if self.unclaimed.contains(account) { 0 } else { self.accounts.get(account).copied().unwrap_or(0) }
    }

    /// The number of accounts that should receive a (non-zero) reward payout.
    pub fn payable_accounts(&self) -> usize {
        self.accounts.iter().filter(|(account, reward)| **reward > 0 && !self.unclaimed.contains(*account)).count()
    }

    /// Amount to be paid to the reserves
    pub fn delta_reserves(&self) -> Lovelace {
        self.delta_reserves
    }

    /// Amount to be paid to the treasury, including the rewards left unclaimed by accounts that
    /// unregistered during the epoch.
    pub fn delta_treasury(&self) -> Lovelace {
        let unclaimed: Lovelace = self.unclaimed.iter().filter_map(|account| self.accounts.get(account)).sum();
        self.delta_treasury + unclaimed
    }

    /// Roll an effective rewards summary back to a computed one by dropping the unclaimed markers.
    /// The shared per-account map is handed over untouched (no copy).
    pub fn to_computed(self) -> Rewards<Computed> {
        Rewards {
            step: PhantomData,
            delta_reserves: self.delta_reserves,
            delta_treasury: self.delta_treasury,
            accounts: self.accounts,
            unclaimed: (),
        }
    }
}

impl From<Rewards<Effective>> for Rewards<Computed> {
    fn from(rewards: Rewards<Effective>) -> Self {
        rewards.to_computed()
    }
}

#[cfg(test)]
mod test {
    use amaru_kernel::Hash;

    use super::*;

    #[test]
    fn test_recovery() {
        let registered = StakeCredential::ScriptHash(Hash::from([1u8; 28]));
        let unregistered = StakeCredential::ScriptHash(Hash::from([2u8; 28]));

        let mut accounts = BTreeMap::new();
        accounts.insert(registered.clone(), 100);
        accounts.insert(unregistered.clone(), 42);

        let delta_reserves = 1_000;
        let delta_treasury = 7;
        let computed_rewards = Rewards::<Computed>::new(delta_reserves, delta_treasury, accounts);

        // `unregistered` unregistered during the epoch, so its rewards can no longer be paid out.
        let effective_rewards =
            Rewards::<Effective>::new(computed_rewards.snapshot(), [registered.clone()].into_iter());

        // The still-registered account is paid its reward; the unregistered one is not (its reward
        // is folded back into the treasury instead).
        assert_eq!(effective_rewards.reward_of(&registered), 100);
        assert_eq!(effective_rewards.reward_of(&unregistered), 0);
        assert_eq!(effective_rewards.payable_accounts(), 1);
        assert_eq!(effective_rewards.delta_reserves(), delta_reserves);
        assert_eq!(effective_rewards.delta_treasury(), delta_treasury + 42);

        // Rolling back drops the unclaimed markers, restoring the original computed rewards.
        let rolled_back = effective_rewards.snapshot().to_computed();
        assert_eq!(rolled_back, computed_rewards, "rollback");
    }
}
