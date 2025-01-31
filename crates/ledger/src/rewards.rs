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


                                                            Computing rewards[^1] using:
                                                            │ - snapshot(e + 1) for
                                                            │     - pool performances
                                                            │     - treasury & reserves
                    Stake is delegated                      │ - snapshot(e) for:
                    │                                       │     - stake distribution
                    │                                       │     - pool parameters
                    │                Using snapshot(e - 1)  │
                    │                for leader schedule    │                 Distributing rewards
                    │                │                      │                 earned from (e)
                    │                │                      │                 │
snapshot(e - 1)     │  snapshot(e)   │    snapshot(e + 1)   │                 │snapshot(e + 2)
              ╽     ╽            ╽   ╽                  ╽   ╽                 ╽╽
━━━━━━━━━━━━╸╸╸╋━━━━━━━━━━━━━━━━╸╸╸╋╸╸╸━━━━━━━━━━━━━━━━╸╸╸╋╸╸╸━━━━━━━━━━━━━━━╸╸╸╋╸╸╸━━━━━━━━>
   e - 1               e                    e + 1                 e + 2              e + 3

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
    kernel::{
        encode_bech32, expect_stake_credential, output_lovelace, output_stake_credential, Epoch,
        Hash, Lovelace, PoolId, PoolParams, StakeCredential, ACTIVE_SLOT_COEFF_INVERSE,
        MAX_LOVELACE_SUPPLY, MONETARY_EXPANSION, OPTIMAL_STAKE_POOLS_COUNT, PLEDGE_INFLUENCE,
        SHELLEY_EPOCH_LENGTH, TREASURY_TAX,
    },
    store::{columns::*, Store},
};
use num::{
    rational::Ratio,
    traits::{One, Zero},
    BigUint,
};
use serde::ser::SerializeStruct;
use std::{collections::BTreeMap, iter};
use tracing::info;

const EVENT_TARGET: &str = "amaru::ledger::state::rewards";

/// A stake distribution snapshot useful for:
///
/// - Leader schedule (in particular the 'pools' field)
/// - Rewards calculation
///
/// Note that the `keys` and `scripts `field only contains _active_ accounts; that is, accounts
/// delegated to a registered stake pool.
#[derive(Debug)]
pub struct StakeDistributionSnapshot {
    /// Epoch number for this snapshot (taken at the end of the epoch)
    epoch: Epoch,

    /// Total stake, in Lovelace, delegated to registered pools
    active_stake: Lovelace,

    /// Mapping of key hash digests to a delegatee (pool) and a rewards amount.
    ///
    /// NOTE:
    /// accounts that have stake but aren't delegated to any pools aren't present in the map.
    keys: BTreeMap<Hash<28>, AccountState>,

    /// Mapping of script hash digests to a delegatee (pool) and a rewards amount.
    ///
    /// NOTE:
    /// accounts that have stake but aren't delegated to any pools aren't present in the map.
    scripts: BTreeMap<Hash<28>, AccountState>,

    /// Mapping of pools to their relative stake & parameters
    pools: BTreeMap<PoolId, PoolState>,
}

impl StakeDistributionSnapshot {
    /// Clompute a new stake distribution snapshot using data available in the `Store`.
    ///
    /// Invariant: The given store is expected to be a snapshot taken at the end of an epoch.
    pub fn new<E>(db: &impl Store<Error = E>) -> Result<Self, E> {
        let mut accounts = BTreeMap::new();
        db.with_accounts(|rows| {
            for (credential, row) in rows {
                if let Some(account) = row.borrow() {
                    if let Some(pool) = account.delegatee {
                        accounts.insert(
                            credential,
                            AccountState {
                                pool,
                                lovelace: account.rewards,
                            },
                        );
                    }
                }
            }
        })?;

        db.with_utxo(|rows| {
            for (_, row) in rows {
                if let Some(output) = row.borrow() {
                    if let Some(credential) = output_stake_credential(output) {
                        let value = output_lovelace(output);
                        accounts
                            .entry(credential)
                            .and_modify(|account| account.lovelace += value);
                    }
                }
            }
        })?;

        let mut pools: BTreeMap<PoolId, PoolState> = BTreeMap::new();
        db.with_pools(|rows| {
            for (pool, row) in rows {
                if let Some(row) = row.borrow() {
                    pools.insert(
                        pool,
                        PoolState {
                            stake: 0,
                            blocks_count: 0,
                            // NOTE: pre-compute margin here (1 - m), which gets used for all
                            // member and leader rewards calculation.
                            margin: safe_ratio(
                                row.current_params.margin.numerator,
                                row.current_params.margin.denominator,
                            ),
                            parameters: row.current_params.clone(),
                        },
                    );
                }
            }
        })?;

        let mut scripts = BTreeMap::new();
        let mut keys = BTreeMap::new();
        let mut active_stake: Lovelace = 0;
        for (credential, account) in accounts.into_iter() {
            pools.entry(account.pool).and_modify(|st| {
                active_stake += &account.lovelace;
                st.stake += &account.lovelace;
            });

            if pools.contains_key(&account.pool) {
                match credential {
                    StakeCredential::ScriptHash(script) => {
                        scripts.insert(script, account);
                    }
                    StakeCredential::AddrKeyhash(key) => {
                        keys.insert(key, account);
                    }
                }
            }
        }

        db.with_block_issuers(|rows| {
            for (_, row) in rows {
                if let Some(issuer) = row.borrow() {
                    pools
                        .entry(issuer.slot_leader)
                        .and_modify(|pool| pool.blocks_count += 1);
                }
            }
        })?;

        let epoch = db.most_recent_snapshot();

        info!(
            name: "stake_distribution.snapshot",
            target: EVENT_TARGET,
            epoch = ?epoch,
            active_stake = ?active_stake,
            accounts = ?(keys.len() + scripts.len()),
            pools = ?pools.len(),
        );

        Ok(StakeDistributionSnapshot {
            epoch,
            active_stake,
            keys,
            scripts,
            pools,
        })
    }
}

impl serde::Serialize for StakeDistributionSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("StakeDistributionSnapshot", 5)?;
        s.serialize_field("epoch", &self.epoch)?;
        s.serialize_field("active_stake", &self.active_stake)?;
        s.serialize_field("keys", &self.keys)?;
        s.serialize_field("scripts", &self.scripts)?;
        let mut pools = self
            .pools
            .iter()
            .map(|(k, v)| (unsafe_encode_pool_id(&k[..]), v))
            .collect::<Vec<_>>();
        pools.sort_by(|a, b| a.0.cmp(&b.0));
        s.serialize_field(
            "pools",
            &pools.into_iter().collect::<BTreeMap<String, &PoolState>>(),
        )?;
        s.end()
    }
}

#[derive(Debug)]
pub struct AccountState {
    pub lovelace: Lovelace,
    pub pool: PoolId,
}

impl AccountState {
    fn from_keys_credentials(
        keys: BTreeMap<Hash<28>, Self>,
    ) -> impl Iterator<Item = (StakeCredential, Self)> {
        keys.into_iter()
            .map(|(key, st)| (StakeCredential::AddrKeyhash(key), st))
    }

    fn from_scripts_credentials(
        scripts: BTreeMap<Hash<28>, Self>,
    ) -> impl Iterator<Item = (StakeCredential, Self)> {
        scripts
            .into_iter()
            .map(|(script, st)| (StakeCredential::ScriptHash(script), st))
    }
}

impl serde::Serialize for AccountState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AccountState", 2)?;
        s.serialize_field("lovelace", &self.lovelace)?;
        s.serialize_field("pool", &unsafe_encode_pool_id(&self.pool[..]))?;
        s.end()
    }
}

#[derive(Debug)]
pub struct PoolState {
    pub blocks_count: u64,
    pub stake: Lovelace,
    pub margin: SafeRatio,
    pub parameters: PoolParams,
}

impl serde::Serialize for PoolState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolState", 3)?;
        s.serialize_field("blocksCount", &self.blocks_count)?;
        s.serialize_field("stake", &self.stake)?;
        s.serialize_field("parameters", &self.parameters)?;
        s.end()
    }
}

impl PoolState {
    pub fn relative_stake(&self, total_stake: Lovelace) -> LovelaceRatio {
        lovelace_ratio(self.stake, total_stake)
    }

    pub fn owner_stake(&self, accounts: &BTreeMap<Hash<28>, AccountState>) -> Lovelace {
        self.parameters
            .owners
            .iter()
            .fold(0, |total, owner| match accounts.get(owner) {
                Some(account) if account.pool == self.parameters.id => total + account.lovelace,
                _ => total,
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
pub struct Pots {
    /// Value, in Lovelace, of the treasury at a given epoch.
    pub treasury: Lovelace,
    /// Value, in Lovelace, of the reserves at a given epoch.
    pub reserves: Lovelace,
    /// Values, in Lovelace, generated from fees during an epoch.
    pub fees: Lovelace,
}

impl From<&pots::Row> for Pots {
    fn from(pots: &pots::Row) -> Pots {
        Pots {
            treasury: pots.treasury,
            reserves: pots.reserves,
            fees: pots.fees,
        }
    }
}

impl serde::Serialize for Pots {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("Pots", 3)?;
        s.serialize_field("treasury", &self.treasury)?;
        s.serialize_field("reserves", &self.reserves)?;
        s.serialize_field("fees", &self.fees)?;
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
        let mut pools = self
            .pools
            .iter()
            .map(|(k, v)| (unsafe_encode_pool_id(&k[..]), v))
            .collect::<Vec<_>>();
        pools.sort_by(|a, b| a.0.cmp(&b.0));
        s.serialize_field(
            "pools",
            &pools
                .into_iter()
                .collect::<BTreeMap<String, &PoolRewards>>(),
        )?;
        s.end()
    }
}

impl RewardsSummary {
    pub fn new<E>(
        db: &impl Store<Error = E>,
        snapshot: StakeDistributionSnapshot,
    ) -> Result<Self, E> {
        let pots = db.with_pots(|entry| Pots::from(entry.borrow()))?;

        let (mut blocks_count, mut blocks_per_pool) = RewardsSummary::count_blocks(db)?;

        let efficiency = safe_ratio(
            blocks_count * ACTIVE_SLOT_COEFF_INVERSE as u64,
            SHELLEY_EPOCH_LENGTH as u64,
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
            snapshot
                .pools
                .iter()
                .fold(0, |effective_rewards, (pool_id, pool)| {
                    let pool_rewards = RewardsSummary::apply_leader_rewards(
                        &mut accounts,
                        &mut blocks_per_pool,
                        blocks_count,
                        available_rewards,
                        total_stake,
                        &snapshot,
                        pool,
                    );

                    let rewards = effective_rewards + pool_rewards.leader;

                    pools.insert(*pool_id, pool_rewards);

                    rewards
                });

        let members = iter::empty()
            .chain(AccountState::from_keys_credentials(snapshot.keys))
            .chain(AccountState::from_scripts_credentials(snapshot.scripts));

        effective_rewards += members.fold(0, |effective_rewards, (credential, account)| {
            let member_rewards = if let Some(pool) = snapshot.pools.get(&account.pool) {
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
        });

        info!(
            name: "rewards.summary",
            target: EVENT_TARGET,
            epoch = ?snapshot.epoch,
            ?efficiency,
            ?incentives,
            ?treasury_tax,
            ?total_rewards,
            ?available_rewards,
            ?effective_rewards,
            pots.reserves = ?pots.reserves,
            pots.treasury = ?pots.treasury,
            pots.fees = ?pots.fees,
        );

        Ok(RewardsSummary {
            epoch: snapshot.epoch,
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

    /// Count blocks produced by pools, returning the total count and map indexed by poolid.
    fn count_blocks<E>(db: &impl Store<Error = E>) -> Result<(u64, BTreeMap<PoolId, u64>), E> {
        let mut total: u64 = 0;
        let mut per_pool: BTreeMap<Hash<28>, u64> = BTreeMap::new();

        db.with_block_issuers(|rows| {
            for (_, row) in rows {
                if let Some(issuer) = row.borrow() {
                    total += 1;
                    per_pool
                        .entry(issuer.slot_leader)
                        .and_modify(|n| *n += 1)
                        .or_insert(1);
                }
            }
        })?;

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
        snapshot: &StakeDistributionSnapshot,
        pool: &PoolState,
    ) -> PoolRewards {
        let owner_stake = pool.owner_stake(&snapshot.keys);

        let rewards_pot = pool.pool_rewards(
            safe_ratio(
                blocks_per_pool
                    .remove(&pool.parameters.id)
                    .unwrap_or_default(),
                blocks_count,
            ),
            available_rewards,
            snapshot.active_stake,
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
    pub fn unclaimed_rewards(self) -> Lovelace {
        self.accounts
            .into_iter()
            .fold(0, |total, (_, rewards)| total + rewards)
    }
}

// -------------------------------------------------------------------- Internal

fn unsafe_encode_pool_id(pool_id: &[u8]) -> String {
    encode_bech32("pool", pool_id)
        .unwrap_or_else(|e| panic!("unable to encode pool id ({pool_id:?}) to bech32: {e:?}"))
}

type SafeRatio = Ratio<BigUint>;

pub fn safe_ratio(numerator: u64, denominator: u64) -> SafeRatio {
    SafeRatio::new(BigUint::from(numerator), BigUint::from(denominator))
}

fn serialize_safe_ratio(r: &SafeRatio) -> String {
    format!("{}/{}", r.numer(), r.denom())
}

type LovelaceRatio = SafeRatio;

pub fn floor_to_lovelace(n: LovelaceRatio) -> Lovelace {
    Lovelace::try_from(n.floor().to_integer()).unwrap_or_else(|_| {
        unreachable!("always fits in a u64; otherwise we've exceeded the max Ada supply.")
    })
}

pub fn lovelace_ratio(numerator: Lovelace, denominator: Lovelace) -> LovelaceRatio {
    LovelaceRatio::new(BigUint::from(numerator), BigUint::from(denominator))
}
