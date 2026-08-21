// Copyright 2024 PRAGMA
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

/*
This module implements the formulas and data structures necessary for rewards and incentives
calculations.

Stakeholders on Cardano can delegate their stake to registered pools, run private pools, or opt
out of the protocol. Non-participation excludes their stake from rewards. Incentives are
primarily monetary, with rewards paid in Ada, aligning financial interests with protocol
adherence to foster a stable, desirable system state.

Rewards are distributed per epoch, drawn from monetary expansion and transaction fees, with a
delay.

Rewards are shared among stake pools based on their contributions, with key refinements to
ensure fairness:

- Rewards are capped for overly large (i.e. saturated) pools to prevent centralization.
- Rewards decrease if pool operators fail to create required blocks as expected.
- Pool operators are compensated via declared costs and margins, with the remainder distributed
  to members.
- Pools with higher owner pledges receive slightly higher rewards, discouraging Sybil attacks
  and stake splitting.

To mitigate chaotic behavior from short-sighted decisions, the system calculates non-myopic
rewards. Wallets rank pools by these rewards, guiding stakeholders toward long-term optimal
behavior. The system stabilizes in a Nash Equilibrium, ensuring no stakeholder has incentive to
deviate from the optimal strategy.

Rewards are calculated and distributed automatically after each epoch, but comes with a delay.

Since Ouroboros is an epoch-based consensus using stake distribution as weight in the (private)
random leader-election procedure, it requires a _stable stake distribution_ to ensure
consistency across leaders. Hence, the stake distribution is considered fixed when an epoch is
over. Changing stake in epoch `e` will only have an effect on the leader schedule in epoch `e + 1`.

Therefore, stake movements on epoch `e` only earns rewards during the calculation in epoch `e + 2`
(since the rewards calculation requires to evaluate the performances of a pool in the previous
epoch).

In addition, given the time needed to compute rewards very much exceeds the computing budget
that a node has an epoch boundary, the calculation is typically done incrementally or spread
during the epoch. This implies that rewards are only distributed on _the next epoch boundary_,
and thus available for withdrawal only in `e + 3`.

Here below is a diagram showing this lifecycle. Note that while step are outline at different
moment from the perspective of the stake movement in epoch `e`, each step is in fact done for
each epoch (related to different snapshots) since it is a continuous cycle.


                                    Pruning retired        Computing rewards[^1] using:
                                    pools                  │ - snapshot(e + 2) for
                                    │                      │     - pool performances
                    Stake is        │                      │     - treasury & reserves
                    delegated       │                      │ - snapshot(e) for:
                    │               │                      │     - stake distribution
                    │               │                      │     - pool parameters
                    │               │Using snapshot(e)     │
                    │               │for leader schedule   │                  Distributing rewards
                    │               ││                     │                  earned from (e)
                    │               ││                     │                  │
    snapshot(e)     │ snapshot(e+1) ││     snapshot(e + 2) │                  │snapshot(e + 3)
              ╽     ╽             ╽ ╽╽                   ╽ ╽                  ╽╽
━━━━━━━━━━━━╸╸╸╋━██━██━██━██━██━╸╸╸╋╸╸╸━██━██━██━██━██━╸╸╸╋╸╸╸━██━██━██━██━██━╸╸╋╸╸╸━██━██━██━>
     e                e + 1          ╿      e + 2 ╿         ╿       e + 3             e + 4
                                     │            │         │
                                     │            Cast vote │
                                     │                      │
                                     │                      Ratifying proposals
                                     │                      using voting power
                                     │                      of (e + 1)
                                     │
                                     Computing voting power for
                                     (e + 1) using state from
                                     beginning of (e + 2)


[^1]: Technically, we need to wait a few slots for the snapshot (e + 1) to stabilise; otherwise
we risk doing an expensive computation which may be rolled back. In practice, the calculation
only starts after 2*k blocks into (e + 2) though conceptually, it boils down to the same thing.

The portions in dotted plots materializes the work done by the ledger at an epoch boundary,
whether the work is considered in the previous epoch or the next depends on what side of the
timeline it is.

- When it appears on the left-hand side, we will say that the computation happens _at the end
  of the epoch_ (once every block for that epoch has been processed, and before any blocks for
  the next epoch is).

- When it appears on the right-hand side, we will say that the computation happens _at the
  beginning of the epoch_ (before any block is ever produced).

The distinction is useful when thinking in terms of snapshots. A snapshot captures the state of
the system at a certain point in time. We always take snapshots _at the end of epochs_, before
certain mutations are applied to the system.
*/

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    Credential, Epoch, GlobalParameters, Lovelace, PoolId, ProtocolParameters, SafeRatio, SortedPairs,
    floor_to_lovelace, safe_ratio,
};
use amaru_observability::info;
use num::{
    BigUint,
    traits::{One, Zero},
};
use serde::ser::SerializeStruct;

use crate::{
    epoch_transition::{Computed, PoolsEpochTransitionUpdates, Rewards},
    store::{Snapshot, StoreError, columns::pots::Row as Pots},
    summary::{AccountState, PoolState, stake_distribution::StakeSummary},
};

impl PoolState {
    pub fn relative_stake(&self, total_stake: Lovelace) -> SafeRatio {
        safe_ratio(self.stake, total_stake)
    }

    pub fn owner_stake(&self, accounts: &SortedPairs<Credential, AccountState>) -> Lovelace {
        self.parameters.owners.iter().fold(0, |total, owner| match accounts.get(&Credential::KeyHash(*owner)) {
            Some(account) if account.pool == Some(self.parameters.id) => total + account.balance,
            _ => total,
        })
    }

    pub fn apparent_performances(&self, blocks_ratio: SafeRatio, active_stake: Lovelace) -> SafeRatio {
        if self.stake.is_zero() {
            SafeRatio::zero()
        } else {
            blocks_ratio * BigUint::from(active_stake) / BigUint::from(self.stake)
        }
    }

    /// Optimal (i.e. maximum) rewards for a pool assuming it is fully saturated and producing
    /// its expected number of blocks.
    ///
    /// The results is then used to calculate the _actual rewards_ based on the pool
    /// performances and its actual saturation level.
    pub fn optimal_rewards(
        &self,
        available_rewards: Lovelace,
        total_stake: Lovelace,
        protocol_parameters: &ProtocolParameters,
    ) -> Lovelace {
        let one = SafeRatio::one();

        let a0 = safe_ratio(
            protocol_parameters.pledge_influence.numerator,
            protocol_parameters.pledge_influence.denominator,
        );

        let z0 = safe_ratio(1, protocol_parameters.optimal_stake_pools_count as u64);

        let relative_pledge = safe_ratio(self.parameters.pledge, total_stake);
        let relative_stake = self.relative_stake(total_stake);

        let r = SafeRatio::from_integer(BigUint::from(available_rewards));
        let p = (&z0).min(&relative_pledge);
        let s = (&z0).min(&relative_stake);

        // R / (1 + a0)
        let left = r / (one + &a0);

        // σ' + p' × a0 × (σ' - p' × (z0 - σ') / z0) / z0
        //               ⎝___________ z0_factor__________⎠
        let right = {
            // (σ' - p' × (z0 - σ') / z0) / z0
            let z0_factor = (s - p * (&z0 - s) / &z0) / &z0;
            s + p * a0 * z0_factor
        };

        // ⌊ (R / (1 + a0)) × (σ' + p' × a0 × (σ' - p' × (z0 - σ') / z0) / z0 ⌋
        //  ⎝____ left ____⎠ ⎝____________________ right ____________________⎠
        floor_to_lovelace(left * right)
    }

    /// The total rewards available to a pool, before it is split between the owner and the
    /// delegators. It is also referred to as the pool rewards pot. Fundamentally, it is the
    /// product of the pool (apparent) performances with its optimal rewards (the case where it is
    /// fully saturated).
    ///
    /// The amount straight to zero if the pool doesn't meet its pledge.
    pub fn pool_rewards(
        &self,
        blocks_ratio: SafeRatio,
        available_rewards: Lovelace,
        active_stake: Lovelace,
        total_stake: Lovelace,
        owner_stake: Lovelace,
        protocol_parameters: &ProtocolParameters,
    ) -> Lovelace {
        if self.parameters.pledge <= owner_stake {
            floor_to_lovelace(
                self.apparent_performances(blocks_ratio, active_stake)
                    * BigUint::from(self.optimal_rewards(available_rewards, total_stake, protocol_parameters)),
            )
        } else {
            0
        }
    }

    /// Portion of the pool rewards that go the owner and increment the pool's registered reward
    /// account. It corresponds to the fixed cost and margin of the pool. The remainder, if any, is
    /// shared amongst delegators.
    pub fn leader_rewards(&self, pool_rewards: Lovelace, owner_stake: Lovelace, total_stake: Lovelace) -> Lovelace {
        let cost: Lovelace = self.parameters.cost;

        if pool_rewards <= cost {
            pool_rewards
        } else {
            let relative_stake = self.relative_stake(total_stake);

            let owner_stake_ratio =
                if total_stake.is_zero() { SafeRatio::zero() } else { safe_ratio(owner_stake, total_stake) };

            // m + (1 - m) × s / σ
            let margin_factor: SafeRatio =
                &self.margin + (SafeRatio::one() - &self.margin) * &owner_stake_ratio / relative_stake;

            // ⌊c + (m + (1 - m) × s / σ) × (R_pool - c)⌋
            //     ⎝___ margin_factor ___⎠
            cost + floor_to_lovelace(margin_factor * BigUint::from(pool_rewards - cost))
        }
    }

    /// Portion of the pool rewards going to a specific member. Note that pool operators receive
    /// leader rewards and are therefore excluded from the member rewards.
    pub fn member_rewards(
        &self,
        member: &Credential,
        pool_rewards: Lovelace,
        member_stake: Lovelace,
        total_stake: Lovelace,
    ) -> Lovelace {
        // NOTE: It may be tempting when seeing the call-site of this function to refactor member
        // to take a `Hash<CREDENTIAL>` instead of a `Credential` directly to make this more uniform.
        //
        // BUT, we know that `owners` cannot be scripts, and a script that would have the same hash
        // as a public key (which is technically near impossible, but still...) would be wrongly
        // labelled as not earning member rewards.
        //
        // So the distinction Script/VerificationKey here *is* useful.
        let is_owner = match member {
            Credential::ScriptHash(..) => false,
            Credential::KeyHash(key) => self.parameters.owners.contains(key),
        };

        if is_owner {
            // Owners don't earn _member rewards_, because they do get _leader rewards_ instead.
            0
        } else {
            let cost: Lovelace = self.parameters.cost;

            if pool_rewards <= cost {
                0
            } else {
                let member_relative_stake = safe_ratio(member_stake, total_stake);

                // ⌊ (1 - m) × (R_pool - c) × t / σ ⌋
                floor_to_lovelace(
                    (SafeRatio::one() - &self.margin) * BigUint::from(pool_rewards - cost) * member_relative_stake
                        / self.relative_stake(total_stake),
                )
            }
        }
    }
}

#[derive(Debug)]
pub struct PoolRewards {
    /// Total rewards available to the pool
    pub pot: Lovelace,
    /// Cut of the rewards going to the pool's leader (operator)
    pub leader: Lovelace,
}

impl serde::Serialize for PoolRewards {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolRewards", 2)?;
        s.serialize_field("pot", &self.pot)?;
        s.serialize_field("leader", &self.leader)?;
        s.end()
    }
}

#[derive(Debug)]
pub struct RewardsSummary {
    /// Epoch number for this summary. Note that the summary is computed during the
    /// following epoch.
    epoch: Epoch,

    /// The amount of Ada taken out of the reserves as incentivies at this particular epoch
    /// (a.k.a ΔR1).
    /// It is so-to-speak, the monetary inflation of the network that fuels the incentives.
    incentives: Lovelace,

    /// Portion of the rewards going to the treasury (irrespective of unallocated pool rewards).
    treasury_tax: Lovelace,

    /// Remaining rewards available to stake pools (and delegators)
    available_rewards: Lovelace,

    /// Effective amount of rewards _actually given out_. The surplus is "sent back"
    /// to the reserves.
    effective_rewards: Lovelace,

    /// Various protocol money pots pertaining to the epoch at the beginning of the rewards calculation.
    pots: Pots,

    /// Per-account state including their rewards.
    accounts: SortedPairs<Credential, AccountState>,

    /// Credentials credited a leader reward, as named by the stake distribution's pool parameters.
    leader_recipients: BTreeSet<Credential>,
}

impl RewardsSummary {
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn new(
        stake_summary: StakeSummary,
        global_parameters: &GlobalParameters,
        protocol_parameters: &ProtocolParameters,
        block_issuers: impl Iterator<Item = PoolId>,
        pots: Pots,
    ) -> Self {
        let (mut blocks_count, mut blocks_per_pool) = RewardsSummary::count_blocks(block_issuers);

        let efficiency =
            safe_ratio(blocks_count * global_parameters.active_slot_coeff_inverse, global_parameters.epoch_length());

        blocks_count = blocks_count.max(1);

        let monetary_expansion_rate = &protocol_parameters.monetary_expansion_rate;
        let monetary_expansion_rate =
            safe_ratio(monetary_expansion_rate.numerator, monetary_expansion_rate.denominator);
        let incentives = floor_to_lovelace(
            (&SafeRatio::one()).min(&efficiency) * &monetary_expansion_rate * BigUint::from(pots.reserves),
        );

        let total_rewards: Lovelace = incentives + pots.fees;

        let treasury_expansion_rate = &protocol_parameters.treasury_expansion_rate;
        let treasury_expansion_rate =
            safe_ratio(treasury_expansion_rate.numerator, treasury_expansion_rate.denominator);
        let treasury_tax: Lovelace = floor_to_lovelace(treasury_expansion_rate * BigUint::from(total_rewards));

        let available_rewards: Lovelace = total_rewards - treasury_tax;

        let total_stake: Lovelace = global_parameters.max_lovelace_supply - pots.reserves;

        let mut leader_recipients: BTreeSet<Credential> = BTreeSet::new();

        let mut pools: BTreeMap<PoolId, PoolRewards> = BTreeMap::new();

        let StakeSummary { stake_distribution, mut accounts } = stake_summary;

        let mut effective_rewards = stake_distribution.pools.iter().fold(0, |effective_rewards, (pool_id, pool)| {
            let pool_rewards = RewardsSummary::apply_leader_rewards(
                &mut accounts,
                &mut leader_recipients,
                &mut blocks_per_pool,
                blocks_count,
                available_rewards,
                stake_distribution.active_stake,
                total_stake,
                pool,
                protocol_parameters,
            );

            let rewards = effective_rewards + pool_rewards.leader;

            pools.insert(*pool_id, pool_rewards);

            rewards
        });

        for (credential, account) in accounts.iter_mut() {
            let opt_pool = account.pool.as_ref().and_then(|pool_id| stake_distribution.pools.get(pool_id));
            effective_rewards += if let Some(pool) = opt_pool {
                RewardsSummary::apply_member_rewards(
                    (credential, account),
                    pool,
                    pools.get(&pool.parameters.id),
                    total_stake,
                )
            } else {
                0
            };
        }

        info!(
            ledger::rewards::SUMMARIZE,
            %efficiency,
            %incentives,
            %treasury_tax,
            %total_rewards,
            %available_rewards,
            %effective_rewards,
            pots_reserves = %pots.reserves,
            pots_treasury = %pots.treasury,
            pots_fees = %pots.fees,
        );

        Self {
            epoch: stake_distribution.epoch,
            incentives,
            treasury_tax,
            available_rewards,
            effective_rewards,
            pots,
            accounts,
            leader_recipients,
        }
    }

    // The test snapshots are powerful, but limited. We define the 'stake distribution' snapshot by
    // looking at the ledger state right after the 'SNAP' rule.
    //
    // The 'rewards' snapshots contains data pertaining to the rewards of a particular epoch. Those
    // rewards are calculated *later*, which makes some element of the snapshot a bit tricky to anchor
    // in time.
    //
    // In particular, the treasury and reserves value used for rewards calculation are the values of
    // the pots *at the moment the calculation begin*.
    //
    // So, for rewards corresponding to an epoch `e`, these calculation begin about `k` blocks within
    // epoch `e + 3`; and are paid out in the transition from `e + 3` to `e + 4` (and thus, available
    // in `e + 4`).
    //
    // BUT: for the 'rewards' snapshot at epoch `e`, we pull the treasury value from our snapshots at
    // `e + 2`; which already includes rewards paid out to accounts as well as the leftovers from any
    // unregistered account, but, does not include leftovers that may come from pool retirements since
    // those are only process _at the beginning of epochs_ (and thus, after the snapshot has been
    // taken).
    //
    // For example, there's a pool retiring in the transition from 176 to 177, with a reward account
    // that's already unregistered. And thus, the deposit is sent back to the treasury. So when the
    // rewards calculation for 174 kicks in later in the epoch, the deposit was already added to the
    // treasury... So it won't be present from our snapshot labeled 176 since it happened BEFORE the
    // beginning of the epoch 177.
    pub fn with_unclaimed_refunds(mut self, db: &impl Snapshot) -> Result<Self, StoreError> {
        let leftovers = PoolsEpochTransitionUpdates::new(db.iter_pools()?, self.epoch + 3).refunds().try_fold(
            0,
            |leftovers, (account, refund)| {
                // TODO: Multi-get here?
                if db.account(account)?.is_none() {
                    return Ok(leftovers + refund);
                }

                Ok::<_, StoreError>(leftovers)
            },
        )?;

        self.pots.treasury += leftovers;

        Ok(self)
    }

    /// Count blocks produced by pools, returning the total count and map indexed by poolid.
    fn count_blocks(iterator: impl Iterator<Item = PoolId>) -> (u64, BTreeMap<PoolId, u64>) {
        let mut total: u64 = 0;
        let mut per_pool: BTreeMap<PoolId, u64> = BTreeMap::new();

        iterator.for_each(|issuer| {
            total += 1;
            per_pool.entry(issuer).and_modify(|n| *n += 1).or_insert(1);
        });

        (total, per_pool)
    }

    fn apply_member_rewards(
        (credential, account): (&Credential, &mut AccountState),
        pool: &PoolState,
        pool_rewards: Option<&PoolRewards>,
        total_stake: Lovelace,
    ) -> Lovelace {
        if let Some(PoolRewards { pot, .. }) = pool_rewards {
            let member_rewards = pool.member_rewards(credential, *pot, account.balance, total_stake);
            if member_rewards > 0 {
                account.rewards += member_rewards;
            }
            member_rewards
        } else {
            0
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn apply_leader_rewards(
        accounts: &mut SortedPairs<Credential, AccountState>,
        leader_recipients: &mut BTreeSet<Credential>,
        blocks_per_pool: &mut BTreeMap<PoolId, u64>,
        blocks_count: u64,
        available_rewards: Lovelace,
        active_stake: Lovelace,
        total_stake: Lovelace,
        pool: &PoolState,
        protocol_parameters: &ProtocolParameters,
    ) -> PoolRewards {
        let owner_stake = pool.owner_stake(accounts);

        let rewards_pot = pool.pool_rewards(
            safe_ratio(blocks_per_pool.remove(&pool.parameters.id).unwrap_or_default(), blocks_count),
            available_rewards,
            active_stake,
            total_stake,
            owner_stake,
            protocol_parameters,
        );

        let rewards_leader = pool.leader_rewards(rewards_pot, owner_stake, total_stake);

        if rewards_leader > 0 {
            let credential = pool.parameters.reward_account.credential();
            leader_recipients.insert(credential);
            if let Some(st) = accounts.get_mut(&credential) {
                st.rewards += rewards_leader;
            } else {
                // NOTE: the reward account needs not be a registered account
                //
                // Nothing above consults the reward account: the pledge is checked against the pool's
                // *owners* and the performance against its *delegators*. So a pool can perfectly well
                // earn a leader reward payable to a credential that has no account, in which case the
                // reward is unclaimed and goes to the treasury instead. That is settled at the epoch
                // boundary, against the recipients recorded here: the pool may change or drop its
                // reward account before the payout, so the registry as it stands then no longer knows
                // who was owed the reward.
                accounts.insert(credential, AccountState::default().with_rewards(rewards_leader))
            }
        }

        PoolRewards { leader: rewards_leader, pot: rewards_pot }
    }

    /// Amount to be depleted from the reserves at the epoch boundary.
    pub fn delta_reserves(&self) -> Lovelace {
        self.incentives + self.effective_rewards - self.available_rewards
    }

    /// Amount to be added to the treasury at the epoch boundary.
    pub fn delta_treasury(&self) -> Lovelace {
        self.treasury_tax
    }

    /// Rewards owed to each credential, whether or not that credential still has an account.
    pub fn accounts(&self) -> &SortedPairs<Credential, AccountState> {
        &self.accounts
    }

    /// Total rewards actually given out, across all credentials.
    pub fn effective_rewards(&self) -> Lovelace {
        self.effective_rewards
    }
}

impl From<RewardsSummary> for Rewards<Computed> {
    fn from(summary: RewardsSummary) -> Self {
        Rewards::<Computed>::new(
            summary.delta_reserves(),
            summary.delta_treasury(),
            summary.effective_rewards,
            summary.accounts,
            summary.leader_recipients,
        )
    }
}

#[cfg(test)]
mod test {
    use amaru_kernel::{
        CertificatePointer, Hash, MAINNET_DEFAULT_PROTOCOL_PARAMETERS, Network, PoolParams, RationalNumber,
        RewardAccount,
    };

    use super::*;
    use crate::summary::stake_distribution::{StakeDistribution, StakeSummary};

    /// A leader reward is credited to the pool's reward account whether or not that credential is a
    /// registered account. Whether it can actually be paid is settled at the epoch boundary, against
    /// the reward accounts collected while ticking the pools.
    #[test]
    fn a_leader_reward_is_credited_to_an_unregistered_reward_account() {
        let pool = pool(1);
        let (accounts, _) = apply_leader_rewards(&pool, stake_summary(&pool, BTreeMap::new()));
        assert!(accounts.contains_key(&credential(1)));
    }

    /// The same holds when the reward account also happens to be a delegator of the pool: it is
    /// credited once for its leader reward here, and once for its member reward elsewhere.
    #[test]
    fn a_leader_reward_is_credited_to_a_registered_reward_account() {
        let pool = pool(1);
        let delegators = BTreeMap::from([(
            credential(1),
            AccountState { balance: STAKE, pool: Some(pool.parameters.id), ..Default::default() },
        )]);

        let (accounts, _) = apply_leader_rewards(&pool, stake_summary(&pool, delegators));

        assert!(accounts.contains_key(&credential(1)));
    }

    /// Crediting a leader reward also records its recipient, so that payability can later be
    /// resolved against the credentials actually owed a reward rather than against the pool
    /// registry as it stands at payout time.
    #[test]
    fn a_leader_reward_records_its_recipient() {
        let pool = pool(1);
        let (_, leader_recipients) = apply_leader_rewards(&pool, stake_summary(&pool, BTreeMap::new()));
        assert_eq!(leader_recipients, BTreeSet::from([credential(1)]));
    }

    /// A pool earning no leader reward records no recipient: anything else would be a wasted
    /// database lookup at the epoch boundary.
    #[test]
    fn a_pool_earning_no_leader_reward_records_no_recipient() {
        let pool = pool(1);
        let mut summary = stake_summary(&pool, BTreeMap::new());

        let mut leader_recipients = BTreeSet::new();
        let mut blocks_per_pool = BTreeMap::new();

        let active_stake = summary.active_stake;

        let rewards = RewardsSummary::apply_leader_rewards(
            &mut summary.accounts,
            &mut leader_recipients,
            &mut blocks_per_pool,
            1,
            1_000_000_000,
            active_stake,
            STAKE,
            &pool,
            &MAINNET_DEFAULT_PROTOCOL_PARAMETERS,
        );

        assert_eq!(rewards.leader, 0, "a pool with no block should earn no leader reward");
        assert!(leader_recipients.is_empty());
        assert!(summary.accounts.values().all(|st| st.rewards == 0));
    }

    // HELPERS

    const STAKE: Lovelace = 1_000_000_000_000;

    fn credential(tag: u8) -> Credential {
        Credential::ScriptHash(Hash::new([tag; 28]))
    }

    /// A pool whose reward account is `credential(tag)`, big enough and productive enough to earn a
    /// leader reward. The margin is 100%, so the whole pot goes to the leader.
    fn pool(tag: u8) -> PoolState {
        PoolState {
            registered_at: CertificatePointer::default(),
            blocks_count: 1,
            stake: STAKE,
            voting_stake: STAKE,
            margin: safe_ratio(1, 1),
            parameters: PoolParams {
                id: PoolId::new([tag; 28]),
                vrf: Hash::new([tag; 32]),
                pledge: 0,
                cost: 0,
                margin: RationalNumber { numerator: 1, denominator: 1 },
                reward_account: RewardAccount::new(Network::Testnet, Credential::ScriptHash(Hash::new([tag; 28]))),
                owners: Vec::new(),
                relays: Vec::new(),
                metadata: None,
            },
            fallback_drep: None,
        }
    }

    fn stake_summary(pool: &PoolState, accounts: BTreeMap<Credential, AccountState>) -> StakeSummary {
        StakeSummary {
            stake_distribution: StakeDistribution {
                epoch: Epoch::from(0),
                treasury: 0,
                reserves: 0,
                active_stake: pool.stake,
                pools_voting_stake: 0,
                dreps_voting_stake: 0,
                pools: BTreeMap::from([(pool.parameters.id, pool.clone())]),
                dreps: BTreeMap::new(),
            },
            accounts: SortedPairs::from(accounts),
        }
    }

    /// Credit a leader reward to `pool`, and report which accounts were credited and which
    /// recipients were recorded.
    fn apply_leader_rewards(
        pool: &PoolState,
        mut stake_summary: StakeSummary,
    ) -> (SortedPairs<Credential, AccountState>, BTreeSet<Credential>) {
        let mut leader_recipients = BTreeSet::new();
        let mut blocks_per_pool = BTreeMap::from([(pool.parameters.id, 1)]);

        let active_stake = stake_summary.active_stake;

        let rewards = RewardsSummary::apply_leader_rewards(
            &mut stake_summary.accounts,
            &mut leader_recipients,
            &mut blocks_per_pool,
            1,
            1_000_000_000,
            active_stake,
            STAKE,
            pool,
            &MAINNET_DEFAULT_PROTOCOL_PARAMETERS,
        );

        assert!(rewards.leader > 0, "the fixture should reward the leader");

        (stake_summary.accounts, leader_recipients)
    }
}
