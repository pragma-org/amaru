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
    summary::governance::{DRepState, GovernanceSummary, ProposalState},
};
use amaru_kernel::{
    encode_bech32, expect_stake_credential, output_stake_credential, DRep, Epoch, HasLovelace,
    Hash, Lovelace, Network, PoolId, PoolParams, ProtocolVersion, StakeCredential,
    ACTIVE_SLOT_COEFF_INVERSE, MAX_LOVELACE_SUPPLY, MONETARY_EXPANSION, OPTIMAL_STAKE_POOLS_COUNT,
    PLEDGE_INFLUENCE, PROTOCOL_VERSION_10, SHELLEY_EPOCH_LENGTH, STAKE_POOL_DEPOSIT, TREASURY_TAX,
};
use iter_borrow::borrowable_proxy::BorrowableProxy;
use num::{
    rational::Ratio,
    traits::{One, Zero},
    BigUint,
};
use serde::ser::SerializeStruct;
use std::collections::{BTreeMap, BTreeSet};
use tracing::info;

const EVENT_TARGET: &str = "amaru::ledger::state::rewards";

/// A stake distribution snapshot useful for:
///
/// - Leader schedule (in particular the 'pools' field)
/// - Rewards calculation
///
/// Note that the `accounts` field only contains _active_ accounts; that is, accounts
/// delegated to a registered stake pool.
#[derive(Debug)]
pub struct StakeDistribution {
    /// Epoch number for this snapshot (taken at the end of the epoch)
    pub epoch: Epoch,

    /// Total stake, in Lovelace, delegated to registered pools
    pub active_stake: Lovelace,

    /// Total voting stake, in Lovelace, corresponding to the total stake assigned to registered
    /// and active delegate representatives.
    pub voting_stake: Lovelace,

    /// Mapping of accounts' stake credentials to their respective state.
    ///
    /// NOTE:
    /// accounts that have stake but aren't delegated to any pools aren't present in the map.
    pub accounts: BTreeMap<StakeCredential, AccountState>,

    /// Mapping of pools to their relative stake & parameters
    pub pools: BTreeMap<PoolId, PoolState>,

    /// Mapping of dreps to their relative stake
    pub dreps: BTreeMap<DRep, DRepState>,
}

impl StakeDistribution {
    /// Compute a new stake distribution snapshot using data available in the `Store`.
    ///
    /// Invariant: The given store is expected to be a snapshot taken at the end of an epoch.
    pub fn new(
        db: &impl Snapshot,
        protocol_version: ProtocolVersion,
        GovernanceSummary {
            mut dreps,
            deposits,
        }: GovernanceSummary,
    ) -> Result<Self, StoreError> {
        let epoch = db.epoch();

        let mut accounts = db
            .iter_accounts()?
            .map(|(credential, account)| {
                (
                    credential.clone(),
                    AccountState {
                        lovelace: account.rewards,
                        pool: account.delegatee,
                        drep: account.drep.clone().and_then(|(drep, since)| match drep {
                            DRep::Abstain | DRep::NoConfidence => Some(drep),
                            DRep::Key { .. } | DRep::Script { .. } => {
                                let DRepState {
                                    registered_at,
                                    previous_deregistration,
                                    ..
                                } = dreps.get(&drep)?;

                                // FIXME: Change this behaviour in protocol 10
                                if protocol_version < PROTOCOL_VERSION_10 {
                                    if &Some(since) > previous_deregistration {
                                        Some(drep)
                                    } else {
                                        None
                                    }
                                } else if &since >= registered_at {
                                    Some(drep)
                                } else {
                                    None
                                }
                            }
                        }),
                    },
                )
            })
            .collect::<BTreeMap<StakeCredential, AccountState>>();

        // TODO: This is the most expensive call in this whole function. It could be made
        // significantly cheaper if we only partially deserialize the UTxOs here. We only need the
        // output's address and *lovelace* value, so we can skip on deserializing the rest of the value, as
        // well as the datum and/or script references if any.
        db.iter_utxos()?.for_each(|(_, output)| {
            if let Ok(Some(credential)) = output_stake_credential(&output) {
                let value = output.lovelace();
                accounts
                    .entry(credential)
                    .and_modify(|account| account.lovelace += value);
            }
        });

        let mut retiring_pools = BTreeSet::new();
        let mut refunds = BTreeMap::new();
        let mut pools = db
            .iter_pools()?
            .map(|(pool, row)| {
                let reward_account = expect_stake_credential(&row.current_params.reward_account);

                // NOTE: We need to tick pool as part of the stake distribution calculation, in
                // order to know whether a pool will retire in the next epoch. This is because,
                // votes ratification happens *after* pools reaping, and thus, nullify voting power
                // of pools that are retiring.
                pools::Row::tick(
                    Box::new(BorrowableProxy::new(Some(row.clone()), |dropped| {
                        if dropped.is_none() {
                            retiring_pools.insert(pool);
                            // FIXME: Store the deposit with the pool, and ensures the same deposit
                            // it returned back.
                            refunds.insert(reward_account, STAKE_POOL_DEPOSIT as Lovelace);
                        }
                    })),
                    epoch + 1,
                );

                (
                    pool,
                    PoolState {
                        stake: 0,
                        voting_stake: 0,
                        blocks_count: 0,
                        // NOTE: pre-compute margin here (1 - m), which gets used for all
                        // member and leader rewards calculation.
                        margin: safe_ratio(
                            row.current_params.margin.numerator,
                            row.current_params.margin.denominator,
                        ),
                        parameters: row.current_params.clone(),
                    },
                )
            })
            .collect::<BTreeMap<Hash<28>, PoolState>>();

        let mut voting_stake: Lovelace = 0;
        let mut active_stake: Lovelace = 0;
        let accounts = accounts
            .into_iter()
            .filter(|(credential, account)| {
                let ProposalState {
                    deposit,
                    valid_until,
                } = deposits
                    .get(credential)
                    .copied()
                    .unwrap_or(ProposalState::default());

                let refund = refunds.get(credential).copied().unwrap_or_default();

                // NOTE: Only accounts delegated to active dreps counts towards the voting stake.
                if let Some(drep) = &account.drep {
                    if let Some(st) = dreps.get_mut(drep) {
                        voting_stake += account.lovelace + deposit + refund;
                        st.stake += account.lovelace + deposit + refund;
                    }
                }

                // NOTE: Only accounts delegated to active pools counts towards the active stake.
                if let Some(pool_id) = account.pool {
                    return match pools.get_mut(&pool_id) {
                        None => false,
                        Some(pool) => {
                            // NOTE: Governance deposits do not count towards the pools' stake.
                            // They are only counted as part of the voting power.
                            active_stake += &account.lovelace;
                            pool.stake += &account.lovelace;

                            // NOTE: Because votes are ratified with an epoch delay and using the
                            // stake distribution _at the beginning of an epoch_ (so, after pool
                            // reap), any pool retiring in the next epoch is considered having no
                            // voting power whatsoever.
                            if !retiring_pools.contains(&pool_id) {
                                pool.voting_stake += account.lovelace;

                                // NOTE: This is subtle, but the pool distribution used for
                                // computing voting power is determined BEFORE refunds or
                                // withdrawal are processed.
                                //
                                // So unlike the DRep voting stake, which already includes those,
                                // we mustn't include the deposit as part of the pool voting stake
                                // for the epoch that immediately follows the expiry.
                                //
                                // Note that the refund is eventually credited in the following
                                // epoch so the deposit is effectively missing from the pools'
                                // voting stake for an entire epoch.
                                if epoch <= valid_until {
                                    pool.voting_stake += deposit;
                                }
                            }
                            true
                        }
                    };
                }

                false
            })
            .collect::<BTreeMap<_, _>>();

        let block_issuers = db.iter_block_issuers()?;
        block_issuers.for_each(|(_, issuer)| {
            pools
                .entry(issuer.slot_leader)
                .and_modify(|pool| pool.blocks_count += 1);
        });

        info!(
            target: EVENT_TARGET,
            epoch = ?epoch,
            accounts = ?accounts.len(),
            pools = ?pools.len(),
            active_stake = ?active_stake,
            dreps = ?dreps.len(),
            voting_stake = ?voting_stake,
            "stake_distribution.snapshot",
        );

        Ok(StakeDistribution {
            epoch,
            active_stake,
            voting_stake,
            accounts,
            pools,
            dreps,
        })
    }

    pub fn for_network(&self, network: Network) -> StakeDistributionForNetwork<'_> {
        StakeDistributionForNetwork(self, network)
    }
}

/// A temporary struct mainly used for serializing a StakeDistribution. This is needed because we
/// need the network id in order to serialize stake credentials as stake address and disambiguate
/// them.
pub struct StakeDistributionForNetwork<'a>(&'a StakeDistribution, Network);

impl serde::Serialize for StakeDistributionForNetwork<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("StakeDistribution", 5)?;
        s.serialize_field("epoch", &self.0.epoch)?;
        s.serialize_field("active_stake", &self.0.active_stake)?;
        s.serialize_field("voting_stake", &self.0.voting_stake)?;
        serialize_map("accounts", &mut s, &self.0.accounts, |credential| {
            encode_stake_credential(self.1, credential)
        })?;
        serialize_map("pools", &mut s, &self.0.pools, encode_pool_id)?;
        serialize_map("dreps", &mut s, &self.0.dreps, encode_drep)?;
        s.end()
    }
}

#[derive(Debug)]
pub struct AccountState {
    pub lovelace: Lovelace,
    pub pool: Option<PoolId>,
    pub drep: Option<DRep>,
}

impl serde::Serialize for AccountState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AccountState", 2)?;
        s.serialize_field("lovelace", &self.lovelace)?;
        s.serialize_field("pool", &self.pool.as_ref().map(encode_pool_id))?;
        s.serialize_field("drep", &self.drep.as_ref().map(encode_drep))?;
        s.end()
    }
}

#[derive(Debug)]
pub struct PoolState {
    /// Number of blocks produced during an epoch by the underlying pool.
    pub blocks_count: u64,

    /// The stake used for verifying the leader-schedule.
    pub stake: Lovelace,

    /// The stake used when counting votes, which includes proposal deposits for proposals whose
    /// refund address is delegated to the underlying pool.
    pub voting_stake: Lovelace,

    /// The pool's margin, as define per its last registration certificate.
    ///
    /// TODO: The margin is already present within the parameters, but is pre-computed here as a
    /// SafeRatio to avoid unnecessary recomputations during rewards calculations. Arguably, it
    /// should just be stored as a `SafeRatio` from within `PoolParams` to begin with!
    pub margin: SafeRatio,

    /// The pool's parameters, as define per its last registration certificate.
    pub parameters: PoolParams,
}

impl serde::Serialize for PoolState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolState", 4)?;
        s.serialize_field("blocks_count", &self.blocks_count)?;
        s.serialize_field("stake", &self.stake)?;
        s.serialize_field("voting_stake", &self.voting_stake)?;
        s.serialize_field("parameters", &self.parameters)?;
        s.end()
    }
}

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
    ) -> Result<Self, StoreError> {
        let pots = db.pots()?;

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

fn encode_pool_id(pool_id: &PoolId) -> String {
    encode_bech32("pool", pool_id.as_slice())
        .unwrap_or_else(|_| unreachable!("human-readable part 'pool' is okay"))
}

/// Serialize a (registerd) DRep to bech32, according to [CIP-0129](https://cips.cardano.org/cip/CIP-0129).
/// The always-Abstain and always-NoConfidence dreps are ignored (i.e. return `None`).
///
/// ```rust
/// use amaru_kernel::{DRep, Hash};
/// use amaru_ledger::summary::rewards::encode_drep;
///
/// let key_drep = DRep::Key(Hash::from(
///   hex::decode("7a719c71d1bc67d2eb4af19f02fd48e7498843d33a22168111344a34")
///     .unwrap()
///     .as_slice()
/// ));
///
/// let script_drep = DRep::Script(Hash::from(
///   hex::decode("429b12461640cefd3a4a192f7c531d8f6c6d33610b727f481eb22d39")
///     .unwrap()
///     .as_slice()
/// ));
///
/// assert_eq!(
///   encode_drep(&DRep::Abstain).as_str(),
///   "abstain",
/// );
///
/// assert_eq!(
///   encode_drep(&DRep::NoConfidence).as_str(),
///   "no_confidence",
/// );
///
/// assert_eq!(
///   encode_drep(&key_drep).as_str(),
///   "drep1yfa8r8r36x7x05htftce7qhafrn5nzzr6vazy95pzy6y5dqac0ss7",
/// );
///
/// assert_eq!(
///   encode_drep(&script_drep).as_str(),
///   "drep1ydpfkyjxzeqvalf6fgvj7lznrk8kcmfnvy9hyl6gr6ez6wgsjaelx",
/// );
/// ```
pub fn encode_drep(drep: &DRep) -> String {
    match drep {
        DRep::Key(hash) => encode_bech32("drep", &[&[0x22], hash.as_slice()].concat()),
        DRep::Script(hash) => encode_bech32("drep", &[&[0x23], hash.as_slice()].concat()),
        DRep::Abstain => Ok("abstain".to_string()),
        DRep::NoConfidence => Ok("no_confidence".to_string()),
    }
    .unwrap_or_else(|_| unreachable!("human-readable part 'drep' is okay"))
}

fn encode_stake_credential(network: Network, credential: &StakeCredential) -> String {
    encode_bech32(
        "stake_test",
        &match credential {
            StakeCredential::AddrKeyhash(hash) => {
                [&[0xe0 | network.value()], hash.as_slice()].concat()
            }
            StakeCredential::ScriptHash(hash) => {
                [&[0xf0 | network.value()], hash.as_slice()].concat()
            }
        },
    )
    .unwrap_or_else(|_| unreachable!("human-readable part 'stake_test' is okay"))
}

fn serialize_map<K, V: serde::ser::Serialize, S: serde::ser::SerializeStruct>(
    field: &'static str,
    s: &mut S,
    m: &BTreeMap<K, V>,
    serialize_key: impl Fn(&K) -> String,
) -> Result<(), S::Error> {
    let mut elems = m
        .iter()
        .map(|(k, v)| (serialize_key(k), v))
        .collect::<Vec<_>>();
    elems.sort_by(|a, b| a.0.cmp(&b.0));
    s.serialize_field(field, &elems.into_iter().collect::<BTreeMap<String, &V>>())
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
