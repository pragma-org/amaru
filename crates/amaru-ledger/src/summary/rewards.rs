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

use crate::{
    store::{columns::*, Snapshot, StoreError},
    summary::{
        safe_ratio,
        serde::{encode_pool_id, serialize_map},
        serialize_safe_ratio,
        stake_distribution::StakeDistribution,
        AccountState, PoolState, Pots, SafeRatio,
    },
};
use amaru_kernel::{
    expect_stake_credential, protocol_parameters::GlobalParameters, Epoch, Hash, Lovelace, PoolId,
    StakeCredential, MAX_LOVELACE_SUPPLY, MONETARY_EXPANSION, OPTIMAL_STAKE_POOLS_COUNT,
    PLEDGE_INFLUENCE, STAKE_POOL_DEPOSIT, TREASURY_TAX,
};
use iter_borrow::borrowable_proxy::BorrowableProxy;
use num::{
    traits::{One, Zero},
    BigUint,
};
use serde::ser::SerializeStruct;
use std::collections::BTreeMap;
use tracing::info;

const EVENT_TARGET: &str = "amaru::ledger::state::rewards";

impl PoolState {
    pub fn relative_stake(&self, total_stake: Lovelace) -> LovelaceRatio {
        lovelace_ratio(self.stake, total_stake)
    }

    pub fn owner_stake(&self, accounts: &BTreeMap<StakeCredential, AccountState>) -> Lovelace {
        self.parameters.owners.iter().fold(0, |total, owner| {
            match accounts.get(&StakeCredential::AddrKeyhash(*owner)) {
                Some(account) if account.pool == Some(self.parameters.id) => {
                    total + account.lovelace
                }
                _ => total,
            }
        })
    }

    pub fn apparent_performances(
        &self,
        blocks_ratio: SafeRatio,
        active_stake: Lovelace,
    ) -> SafeRatio {
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
    pub fn optimal_rewards(&self, available_rewards: Lovelace, total_stake: Lovelace) -> Lovelace {
        let one = SafeRatio::one();
        let a0 = &*PLEDGE_INFLUENCE;
        let z0 = safe_ratio(1, OPTIMAL_STAKE_POOLS_COUNT as u64);

        let relative_pledge = lovelace_ratio(self.parameters.pledge, total_stake);
        let relative_stake = self.relative_stake(total_stake);

        let r = SafeRatio::from_integer(BigUint::from(available_rewards));
        let p = (&z0).min(&relative_pledge);
        let s = (&z0).min(&relative_stake);

        // R / (1 + a0)
        let left = r / (one + a0);

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
    ) -> Lovelace {
        if self.parameters.pledge <= owner_stake {
            floor_to_lovelace(
                self.apparent_performances(blocks_ratio, active_stake)
                    * BigUint::from(self.optimal_rewards(available_rewards, total_stake)),
            )
        } else {
            0
        }
    }

    /// Portion of the pool rewards that go the owner and increment the pool's registered reward
    /// account. It corresponds to the fixed cost and margin of the pool. The remainder, if any, is
    /// shared amongst delegators.
    pub fn leader_rewards(
        &self,
        pool_rewards: Lovelace,
        owner_stake: Lovelace,
        total_stake: Lovelace,
    ) -> Lovelace {
        let cost: Lovelace = self.parameters.cost;

        if pool_rewards <= cost {
            pool_rewards
        } else {
            let relative_stake = self.relative_stake(total_stake);

            let owner_stake_ratio = if total_stake.is_zero() {
                LovelaceRatio::zero()
            } else {
                lovelace_ratio(owner_stake, total_stake)
            };

            // m + (1 - m) × s / σ
            let margin_factor: SafeRatio = &self.margin
                + (SafeRatio::one() - &self.margin) * &owner_stake_ratio / relative_stake;

            // ⌊c + (m + (1 - m) × s / σ) × (R_pool - c)⌋
            //     ⎝___ margin_factor ___⎠
            cost + floor_to_lovelace(margin_factor * BigUint::from(pool_rewards - cost))
        }
    }

    /// Portion of the pool rewards going to a specific member. Note that pool operators receive
    /// leader rewards and are therefore excluded from the member rewards.
    pub fn member_rewards(
        &self,
        member: &StakeCredential,
        pool_rewards: Lovelace,
        member_stake: Lovelace,
        total_stake: Lovelace,
    ) -> Lovelace {
        // NOTE: It may be tempting when seeing the call-site of this function to refactor member
        // to take a `Hash<28>` instead of a `StakeCredential` directly to make this more uniform.
        //
        // BUT, we know that `owners` cannot be scripts, and a script that would have the same hash
        // as a public key (which is technically near impossible, but still...) would be wrongly
        // labelled as not earning member rewards.
        //
        // So the distinction Script/VerificationKey here *is* useful.
        let is_owner = match member {
            StakeCredential::ScriptHash(..) => false,
            StakeCredential::AddrKeyhash(key) => self.parameters.owners.contains(key),
        };

        if is_owner {
            // Owners don't earn _member rewards_, because they do get _leader rewards_ instead.
            0
        } else {
            let cost: Lovelace = self.parameters.cost;

            if pool_rewards <= cost {
                0
            } else {
                let member_relative_stake = lovelace_ratio(member_stake, total_stake);

                // ⌊ (1 - m) × (R_pool - c) × t / σ ⌋
                floor_to_lovelace(
                    (SafeRatio::one() - &self.margin)
                        * BigUint::from(pool_rewards - cost)
                        * member_relative_stake
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

    /// The ratio of total blocks produced in the epoch, over the expected number of blocks
    /// (determined by protocol parameters).
    efficiency: SafeRatio,

    /// The amount of Ada taken out of the reserves as incentivies at this particular epoch
    /// (a.k.a ΔR1).
    /// It is so-to-speak, the monetary inflation of the network that fuels the incentives.
    incentives: Lovelace,

    /// Total amount of rewards available before the treasury tax.
    /// In particular, we have:
    ///
    ///   total_rewards = treasury_tax + available_rewards
    total_rewards: Lovelace,

    /// Portion of the rewards going to the treasury (irrespective of unallocated pool rewards).
    treasury_tax: Lovelace,

    /// Remaining rewards available to stake pools (and delegators)
    available_rewards: Lovelace,

    /// Effective amount of rewards _actually given out_. The surplus is "sent back"
    /// to the reserves.
    effective_rewards: Lovelace,

    /// Various protocol money pots pertaining to the epoch at the beginning of the rewards calculation.
    pots: Pots,

    /// Per-pool rewards determined from their (apparent) performances, available rewards and
    /// relative stake.
    pools: BTreeMap<PoolId, PoolRewards>,

    /// Per-account rewards, determined from their relative stake and their delegatee.
    accounts: BTreeMap<StakeCredential, Lovelace>,
}

impl serde::Serialize for RewardsSummary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("RewardsSummary", 8)?;
        s.serialize_field("epoch", &self.epoch)?;
        s.serialize_field("efficiency", &serialize_safe_ratio(&self.efficiency))?;
        s.serialize_field("incentives", &self.incentives)?;
        s.serialize_field("total_rewards", &self.total_rewards)?;
        s.serialize_field("treasury_tax", &self.treasury_tax)?;
        s.serialize_field("available_rewards", &self.available_rewards)?;
        s.serialize_field("pots", &self.pots)?;
        serialize_map("pools", &mut s, &self.pools, encode_pool_id)?;
        s.end()
    }
}

impl RewardsSummary {
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn new(
        db: &impl Snapshot,
        stake_distribution: StakeDistribution,
        global_parameters: &GlobalParameters,
    ) -> Result<Self, StoreError> {
        let pots = db.pots()?;

        let (mut blocks_count, mut blocks_per_pool) = RewardsSummary::count_blocks(db)?;

        let efficiency = safe_ratio(
            blocks_count * global_parameters.active_slot_coeff_inverse as u64,
            global_parameters.shelley_epoch_length() as u64,
        );

        blocks_count = blocks_count.max(1);

        let incentives = floor_to_lovelace(
            (&SafeRatio::one()).min(&efficiency)
                * &*MONETARY_EXPANSION
                * BigUint::from(pots.reserves),
        );

        let total_rewards: Lovelace = incentives + pots.fees;

        let treasury_tax: Lovelace =
            floor_to_lovelace(&*TREASURY_TAX * BigUint::from(total_rewards));

        let available_rewards: Lovelace = total_rewards - treasury_tax;

        let total_stake: Lovelace = MAX_LOVELACE_SUPPLY - pots.reserves;

        let mut accounts: BTreeMap<StakeCredential, Lovelace> = BTreeMap::new();

        let mut pools: BTreeMap<PoolId, PoolRewards> = BTreeMap::new();

        let mut effective_rewards =
            stake_distribution
                .pools
                .iter()
                .fold(0, |effective_rewards, (pool_id, pool)| {
                    let pool_rewards = RewardsSummary::apply_leader_rewards(
                        &mut accounts,
                        &mut blocks_per_pool,
                        blocks_count,
                        available_rewards,
                        total_stake,
                        &stake_distribution,
                        pool,
                    );

                    let rewards = effective_rewards + pool_rewards.leader;

                    pools.insert(*pool_id, pool_rewards);

                    rewards
                });

        effective_rewards += stake_distribution.accounts.into_iter().fold(
            0,
            |effective_rewards, (credential, account)| {
                let opt_pool = account
                    .pool
                    .as_ref()
                    .and_then(|pool_id| stake_distribution.pools.get(pool_id));

                let member_rewards = if let Some(pool) = opt_pool {
                    RewardsSummary::apply_member_rewards(
                        &mut accounts,
                        pool,
                        pools.get(&pool.parameters.id),
                        total_stake,
                        credential,
                        account,
                    )
                } else {
                    0
                };

                effective_rewards + member_rewards
            },
        );

        info!(
            target: EVENT_TARGET,
            epoch = ?stake_distribution.epoch,
            %efficiency,
            ?incentives,
            ?treasury_tax,
            ?total_rewards,
            ?available_rewards,
            ?effective_rewards,
            pots.reserves = ?pots.reserves,
            pots.treasury = ?pots.treasury,
            pots.fees = ?pots.fees,
            "rewards.summary",
        );

        Ok(RewardsSummary {
            epoch: stake_distribution.epoch,
            efficiency,
            incentives,
            total_rewards,
            treasury_tax,
            available_rewards,
            effective_rewards,
            pots,
            pools,
            accounts,
        })
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
        let leftovers = db.iter_pools()?.try_fold(0, |leftovers, (_, row)| {
            if let Some(account) = pools::Row::tick(
                Box::new(BorrowableProxy::new(Some(row), |_| {})),
                self.epoch + 3,
            ) {
                if db.account(&account)?.is_none() {
                    return Ok::<_, StoreError>(leftovers + STAKE_POOL_DEPOSIT);
                }
            }

            Ok(leftovers)
        })?;

        self.pots.treasury += leftovers;

        Ok(self)
    }

    /// Count blocks produced by pools, returning the total count and map indexed by poolid.
    fn count_blocks(db: &impl Snapshot) -> Result<(u64, BTreeMap<PoolId, u64>), StoreError> {
        let mut total: u64 = 0;
        let mut per_pool: BTreeMap<Hash<28>, u64> = BTreeMap::new();

        let block_issuers = db.iter_block_issuers()?.map(|(_, issuer)| issuer);
        block_issuers.for_each(|issuer| {
            total += 1;
            per_pool
                .entry(issuer.slot_leader)
                .and_modify(|n| *n += 1)
                .or_insert(1);
        });

        Ok((total, per_pool))
    }

    fn apply_member_rewards(
        accounts: &mut BTreeMap<StakeCredential, Lovelace>,
        pool: &PoolState,
        pool_rewards: Option<&PoolRewards>,
        total_stake: Lovelace,
        credential: StakeCredential,
        st: AccountState,
    ) -> Lovelace {
        if let Some(PoolRewards { pot, .. }) = pool_rewards {
            let member_rewards = pool.member_rewards(&credential, *pot, st.lovelace, total_stake);
            if member_rewards > 0 {
                accounts
                    .entry(credential)
                    .and_modify(|rewards| *rewards += member_rewards)
                    .or_insert(member_rewards);
            }
            member_rewards
        } else {
            0
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_leader_rewards(
        accounts: &mut BTreeMap<StakeCredential, Lovelace>,
        blocks_per_pool: &mut BTreeMap<PoolId, u64>,
        blocks_count: u64,
        available_rewards: Lovelace,
        total_stake: Lovelace,
        stake_distribution: &StakeDistribution,
        pool: &PoolState,
    ) -> PoolRewards {
        let owner_stake = pool.owner_stake(&stake_distribution.accounts);

        let rewards_pot = pool.pool_rewards(
            safe_ratio(
                blocks_per_pool
                    .remove(&pool.parameters.id)
                    .unwrap_or_default(),
                blocks_count,
            ),
            available_rewards,
            stake_distribution.active_stake,
            total_stake,
            owner_stake,
        );

        let rewards_leader = pool.leader_rewards(rewards_pot, owner_stake, total_stake);

        accounts
            .entry(expect_stake_credential(&pool.parameters.reward_account))
            .and_modify(|rewards| *rewards += rewards_leader)
            .or_insert(rewards_leader);

        PoolRewards {
            leader: rewards_leader,
            pot: rewards_pot,
        }
    }

    /// Amount to be depleted from the reserves at the epoch boundary.
    pub fn delta_reserves(&self) -> Lovelace {
        self.incentives + self.effective_rewards - self.available_rewards
    }

    /// Amount to be added to the treasury at the epoch boundary.
    pub fn delta_treasury(&self) -> Lovelace {
        self.treasury_tax
    }

    /// Fetch and remove from the summary rewards pertaining to a given account, if any.
    pub fn extract_rewards(&mut self, account: &StakeCredential) -> Option<Lovelace> {
        self.accounts.remove(account)
    }

    /// Return leftovers rewards that couldn't be allocated to account because they no longer
    /// exist. This method consumes (i.e. takes ownership) of the item because it is meant to be
    /// called last.
    pub fn unclaimed_rewards(&self) -> Lovelace {
        self.accounts
            .iter()
            .fold(0, |total, (_, rewards)| total + rewards)
    }
}

// -------------------------------------------------------------------- Internal

type LovelaceRatio = SafeRatio;

fn floor_to_lovelace(n: LovelaceRatio) -> Lovelace {
    Lovelace::try_from(n.floor().to_integer()).unwrap_or_else(|_| {
        unreachable!("always fits in a u64; otherwise we've exceeded the max Ada supply.")
    })
}

fn lovelace_ratio(numerator: Lovelace, denominator: Lovelace) -> LovelaceRatio {
    LovelaceRatio::new(BigUint::from(numerator), BigUint::from(denominator))
}
