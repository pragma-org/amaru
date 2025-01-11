//! This module implements the formulas and data structures necessary for rewards and incentives
//! calculations.
//!
//! Stakeholders on Cardano can delegate their stake to registered pools, run private pools, or opt
//! out of the protocol. Non-participation excludes their stake from rewards. Incentives are
//! primarily monetary, with rewards paid in Ada, aligning financial interests with protocol
//! adherence to foster a stable, desirable system state.
//!
//! Rewards are distributed per epoch, drawn from monetary expansion and transaction fees, with a
//! delay.
//!
//! Rewards are shared among stake pools based on their contributions, with key refinements to
//! ensure fairness:
//!
//! - Rewards are capped for overly large (i.e. saturated) pools to prevent centralization.
//! - Rewards decrease if pool operators fail to create required blocks as expected.
//! - Pool operators are compensated via declared costs and margins, with the remainder distributed
//!   to members.
//! - Pools with higher owner pledges receive slightly higher rewards, discouraging Sybil attacks
//!   and stake splitting.
//!
//! To mitigate chaotic behavior from short-sighted decisions, the system calculates non-myopic
//! rewards. Wallets rank pools by these rewards, guiding stakeholders toward long-term optimal
//! behavior. The system stabilizes in a Nash Equilibrium, ensuring no stakeholder has incentive to
//! deviate from the optimal strategy.
//!
//! Rewards are calculated and distributed automatically after each epoch, but comes with a delay.
//!
//! Since Ouroboros is an epoch-based consensus using stake distribution as weight in the (private)
//! random leader-election procedure, it requires a _stable stake distribution_ to ensure
//! consistency across leaders. Hence, the stake distribution is considered fixed when an epoch is
//! over. Changing stake in epoch `e` will only have an effect on the leader schedule in epoch `e + 1`.
//!
//! Therefore, stake movements on epoch `e` only earns rewards during the calculation in epoch `e + 2`
//! (since the rewards calculation requires to evaluate the performances of a pool in the previous
//! epoch).
//!
//! In addition, given the time needed to compute rewards very much exceeds the computing budget
//! that a node has an epoch boundary, the calculation is typically done incrementally or spread
//! during the epoch. This implies that rewards are only distributed on _the next epoch boundary_,
//! and thus available for withdrawal only in `e + 3`.
//!
//! Here below is a diagram showing this lifecycle. Note that while step are outline at different
//! moment from the perspective of the stake movement in epoch `e`, each step is in fact done for
//! each epoch (related to different snapshots) since it is a continuous cycle.
//!
//!
//!                                                             Computing rewards using:
//!                                                             │ - snapshot(e + 1) for
//!                                                             │     - pool performances
//!                                                             │     - treasury & reserves
//!                     Stake is delegated                      │ - snapshot(e) for:
//!                     │                                       │     - stake distribution
//!                     │                                       │     - pool parameters
//!                     │                Using snapshot(e - 1)  │
//!                     │                for leader schedule    │                 Distributing rewards
//!                     │                │                      │                 earned from (e)
//!                     │                │                      │                 │
//! snapshot(e - 1)     │  snapshot(e)   │    snapshot(e + 1)   │                 │snapshot(e + 2)
//!               ╽     ╽            ╽   ╽                  ╽   ╽                 ╽╽
//! ━━━━━━━━━━━━╸╸╸╋━━━━━━━━━━━━━━━━╸╸╸╋╸╸╸━━━━━━━━━━━━━━━━╸╸╸╋╸╸╸━━━━━━━━━━━━━━━╸╸╸╋╸╸╸━━━━━━━━>
//!    e - 1               e                    e + 1                 e + 2              e + 3
//!
//! The portions in dotted plots materializes the work done by the ledger at an epoch boundary,
//! whether the work is considered in the previous epoch or the next depends on what side of the
//! timeline it is.
//!
//! - When it appears on the left-hand side, we will say that the computation happens _at the end
//!   of the epoch_ (once every block for that epoch has been processed, and before any blocks for
//!   the next epoch is).
//!
//! - When it appears on the right-hand side, we will say that the computation happens _at the
//!   beginning of the epoch_ (before any block is ever produced).
//!
//! The distinction is useful when thinking in terms of snapshots. A snapshot captures the state of
//! the system at a certain point in time. We always take snapshots _at the end of epochs_, before
//! certain mutations are applied to the system.

use crate::ledger::{
    kernel::{
        encode_bech32, output_lovelace, output_stake_credential, Epoch, Hash, PoolId, PoolParams,
        StakeCredential, ACTIVE_SLOT_COEFF_INVERSE, MAX_LOVELACE_SUPPLY, MONETARY_EXPANSION,
        OPTIMAL_STAKE_POOLS_COUNT, PLEDGE_INFLUENCE, SHELLEY_EPOCH_LENGTH, TREASURY_TAX,
    },
    store::{columns::*, Store},
};
use num::{
    traits::{One, Zero},
    BigInt, BigRational,
};
use serde::ser::SerializeStruct;
use std::collections::BTreeMap;

/// A stake distribution snapshot useful for:
///
/// - Leader schedule (in particular the 'pools' field)
/// - Rewards calculation
///
/// Note that the `keys` and `scripts `field only contains _active_ accounts; that is, accounts
/// delegated to a registered stake pool.
#[derive(Debug)]
pub struct StakeDistributionSnapshot {
    pub epoch: Epoch,
    pub active_stake: BigInt,
    pub keys: BTreeMap<Hash<28>, AccountState>,
    pub scripts: BTreeMap<Hash<28>, AccountState>,
    pub pools: BTreeMap<PoolId, PoolState>,
}

impl StakeDistributionSnapshot {
    /// Clompute a new stake distribution snapshot using data available in the `Store`.
    ///
    /// Invariant: The given store is expected to be a snapshot taken at the end of an epoch.
    pub fn new<E>(db: &impl Store<Error = E>) -> Result<Self, E> {
        // TODO: Avoid creating this intermediate map, and directly create the keys/scripts ones.
        // Then, when looking for _active accounts_ delegated to pools, we can prune those not
        // referenced anywhere.
        let mut accounts = BTreeMap::new();
        db.with_accounts(|rows| {
            for (credential, row) in rows {
                if let Some(account) = row.borrow() {
                    if let Some(pool) = account.delegatee {
                        accounts.insert(
                            credential,
                            AccountState {
                                pool,
                                lovelace: BigInt::from(account.rewards),
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
                            stake: BigInt::ZERO,
                            blocks_count: 0,
                            parameters: row.current_params.clone(),
                        },
                    );
                }
            }
        })?;

        let mut scripts = BTreeMap::new();
        let mut keys = BTreeMap::new();
        let mut active_stake: BigInt = BigInt::ZERO;
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

        Ok(StakeDistributionSnapshot {
            epoch: db.most_recent_snapshot(),
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
        s.serialize_field("active_stake", &serialize_bigint(&self.active_stake))?;
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
    pub lovelace: BigInt,
    pub pool: PoolId,
}

impl serde::Serialize for AccountState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AccountState", 2)?;
        s.serialize_field("lovelace", &serialize_bigint(&self.lovelace))?;
        s.serialize_field("pool", &unsafe_encode_pool_id(&self.pool[..]))?;
        s.end()
    }
}

#[derive(Debug)]
pub struct PoolState {
    pub blocks_count: u64,
    pub stake: BigInt,
    pub parameters: PoolParams,
}

impl serde::Serialize for PoolState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolState", 3)?;
        s.serialize_field("blocksCount", &self.blocks_count)?;
        s.serialize_field("stake", &serialize_bigint(&self.stake))?;
        s.serialize_field("parameters", &self.parameters)?;
        s.end()
    }
}

impl PoolState {
    pub fn relative_stake(&self, total_stake: &BigInt) -> BigRational {
        BigRational::new(self.stake.clone(), total_stake.clone())
    }

    pub fn owner_stake(&self, accounts: &BTreeMap<Hash<28>, AccountState>) -> BigInt {
        self.parameters
            .owners
            .iter()
            .fold(BigInt::ZERO, |total, owner| match accounts.get(owner) {
                Some(account) if account.pool == self.parameters.id => total + &account.lovelace,
                _ => total,
            })
    }

    pub fn apparent_performances(
        &self,
        blocks_ratio: BigRational,
        active_stake: &BigInt,
    ) -> BigRational {
        if self.stake.is_zero() {
            BigRational::zero()
        } else {
            blocks_ratio * active_stake / &self.stake
        }
    }

    /// Optimal (i.e. maximum) rewards for a pool assuming it is fully saturated and producing
    /// its expected number of blocks.
    ///
    /// The results is then used to calculate the _actual rewards_ based on the pool
    /// performances and its actual saturation level.
    pub fn optimal_rewards(&self, available_rewards: &BigInt, total_stake: &BigInt) -> BigInt {
        let one = BigRational::one();
        let a0 = &*PLEDGE_INFLUENCE;
        let z0 = BigRational::new(1.into(), OPTIMAL_STAKE_POOLS_COUNT.into());

        let relative_pledge = BigRational::new(self.parameters.pledge.into(), total_stake.clone());
        let relative_stake = self.relative_stake(total_stake);

        let r = BigRational::from_integer(available_rewards.clone());
        let p = (&z0).min(&relative_pledge);
        let s = (&z0).min(&relative_stake);

        // R / (1 + a0)
        let left = r / (one + a0);

        // σ' + p' * a0 * z0_factor
        let right = {
            // (σ' - p' * (z0 - σ') / z0) / z0
            let z0_factor = (s - p * (&z0 - s) / &z0) / &z0;
            s + p * a0 * z0_factor
        };

        (left * right).floor().to_integer()
    }

    pub fn pool_rewards(
        &self,
        blocks_ratio: BigRational,
        available_rewards: &BigInt,
        active_stake: &BigInt,
        total_stake: &BigInt,
        owner_stake: &BigInt,
    ) -> BigInt {
        if &BigInt::from(self.parameters.pledge) <= owner_stake {
            (self.apparent_performances(blocks_ratio, active_stake)
                * self.optimal_rewards(available_rewards, total_stake))
            .floor()
            .to_integer()
        } else {
            BigInt::ZERO
        }
    }

    pub fn leader_rewards(
        &self,
        pool_rewards: BigInt,
        owner_stake: BigInt,
        total_stake: BigInt,
    ) -> BigInt {
        let cost: BigInt = self.parameters.cost.into();

        let margin: BigRational = BigRational::new(
            self.parameters.margin.numerator.into(),
            self.parameters.margin.denominator.into(),
        );

        if pool_rewards <= cost {
            pool_rewards
        } else {
            let member_rewards: BigInt = pool_rewards - &cost;

            let relative_stake = self.relative_stake(&total_stake);

            let owner_stake_ratio = if total_stake.is_zero() {
                BigRational::zero()
            } else {
                BigRational::new(owner_stake, total_stake)
            };

            let one = BigRational::one();

            let margin_factor: BigRational =
                &margin + (one - &margin) * &owner_stake_ratio / relative_stake;

            cost + (margin_factor * member_rewards).floor().to_integer()
        }
    }
}

#[derive(Debug)]
pub struct PoolRewards {
    /// Total rewards available to the pool
    pub pot: BigInt,
    /// Cut of the rewards going to the pool's leader (operator)
    pub leader: BigInt,
}

impl serde::Serialize for PoolRewards {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolRewards", 2)?;
        s.serialize_field("pot", &serialize_bigint(&self.pot))?;
        s.serialize_field("leader", &serialize_bigint(&self.leader))?;
        s.end()
    }
}

#[derive(Debug)]
pub struct Pots {
    /// Value, in Lovelace, of the treasury at a given epoch.
    pub treasury: BigInt,
    /// Value, in Lovelace, of the reserves at a given epoch.
    pub reserves: BigInt,
    /// Values, in Lovelace, generated from fees during an epoch.
    pub fees: BigInt,
}

impl From<pots::Row> for Pots {
    fn from(pots: pots::Row) -> Pots {
        Pots {
            treasury: pots.treasury.into(),
            reserves: pots.reserves.into(),
            fees: pots.fees.into(),
        }
    }
}

impl serde::Serialize for Pots {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("Pots", 3)?;
        s.serialize_field("treasury", &serialize_bigint(&self.treasury))?;
        s.serialize_field("reserves", &serialize_bigint(&self.reserves))?;
        s.serialize_field("fees", &serialize_bigint(&self.fees))?;
        s.end()
    }
}

#[derive(Debug)]
pub struct RewardsSummary {
    /// Epoch number for this summary. Note that the summary is computed during the
    /// following epoch.
    pub epoch: Epoch,

    /// The ratio of total blocks produced in the epoch, over the expected number of blocks
    /// (determined by protocol parameters).
    pub efficiency: BigRational,

    /// The amount of Ada taken out of the reserves as incentivies at this particular epoch
    /// (a.k.a ΔR1).
    /// It is so-to-speak, the monetary inflation of the network that fuels the incentives.
    pub incentives: BigInt,

    /// Total amount of rewards available before the treasury tax.
    /// In particular, we have:
    ///
    ///   total_rewards = treasury_tax + available_rewards
    pub total_rewards: BigInt,

    /// Portion of the rewards going to the treasury (irrespective of unallocated pool rewards).
    pub treasury_tax: BigInt,

    /// Remaining rewards available to stake pools (and delegators)
    pub available_rewards: BigInt,

    /// Various protocol money pots pertaining to the epoch
    pub pots: Pots,

    /// Per-pool rewards determined from their (apparent) performances, available rewards and
    /// relative stake.
    pub pools: BTreeMap<PoolId, PoolRewards>,
}

impl serde::Serialize for RewardsSummary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("RewardsSummary", 8)?;
        s.serialize_field("epoch", &self.epoch)?;
        s.serialize_field("efficiency", &serialize_ratio(&self.efficiency))?;
        s.serialize_field("incentives", &serialize_bigint(&self.incentives))?;
        s.serialize_field("total_rewards", &serialize_bigint(&self.total_rewards))?;
        s.serialize_field("treasury_tax", &serialize_bigint(&self.treasury_tax))?;
        s.serialize_field(
            "available_rewards",
            &serialize_bigint(&self.available_rewards),
        )?;
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
        snapshot: &StakeDistributionSnapshot,
    ) -> Result<Self, E> {
        let epoch = db.most_recent_snapshot() - 2;

        let pots: Pots = db.pots()?.into();

        let mut blocks_count = BigInt::ZERO;
        let mut pools_blocks = BTreeMap::new();
        db.with_block_issuers(|rows| {
            for (_, row) in rows {
                if let Some(issuer) = row.borrow() {
                    blocks_count += 1;
                    pools_blocks
                        .entry(issuer.slot_leader)
                        .and_modify(|n| *n += 1)
                        .or_insert(BigInt::one());
                }
            }
        })?;

        let one = BigRational::one();

        let efficiency = BigRational::new(
            &blocks_count * ACTIVE_SLOT_COEFF_INVERSE,
            SHELLEY_EPOCH_LENGTH.into(),
        );

        blocks_count = blocks_count.max(BigInt::one());

        let incentives = ((&one).min(&efficiency) * &*MONETARY_EXPANSION * &pots.reserves)
            .floor()
            .to_integer();

        let total_rewards: BigInt = &incentives + &pots.fees;

        let treasury_tax: BigInt = (&*TREASURY_TAX * &total_rewards).floor().to_integer();

        let available_rewards: BigInt = &total_rewards - &treasury_tax;

        let total_stake: BigInt = MAX_LOVELACE_SUPPLY - &pots.reserves;

        let pools = snapshot
            .pools
            .iter()
            .fold(BTreeMap::new(), |mut pools, (pool_id, pool)| {
                let owner_stake = pool.owner_stake(&snapshot.keys);

                let rewards_pot = pool.pool_rewards(
                    BigRational::new(
                        pools_blocks.remove(pool_id).unwrap_or_default(),
                        blocks_count.clone(),
                    ),
                    &available_rewards,
                    &snapshot.active_stake,
                    &total_stake,
                    &owner_stake,
                );

                pools.insert(
                    *pool_id,
                    PoolRewards {
                        leader: pool.leader_rewards(
                            rewards_pot.clone(),
                            owner_stake,
                            total_stake.clone(),
                        ),
                        pot: rewards_pot,
                    },
                );

                pools
            });

        Ok(RewardsSummary {
            epoch,
            efficiency,
            incentives,
            total_rewards,
            treasury_tax,
            available_rewards,
            pots,
            pools,
        })
    }
}

// -------------------------------------------------------------------- Internal

fn serialize_ratio(r: &BigRational) -> String {
    format!("{}/{}", r.numer(), r.denom())
}

fn serialize_bigint(n: &BigInt) -> i128 {
    i128::try_from(n).unwrap()
}

fn unsafe_encode_pool_id(pool_id: &[u8]) -> String {
    encode_bech32("pool", pool_id)
        .unwrap_or_else(|e| panic!("unable to encode pool id ({pool_id:?}) to bech32: {e:?}"))
}

// -------------------------------------------------------------------- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::store::rocksdb::RocksDB;
    use std::path::PathBuf;

    const LEDGER_DB: &str = "../../ledger.db";

    fn fixture_stake_distribution_snapshot(epoch: Epoch) -> StakeDistributionSnapshot {
        let db = RocksDB::from_snapshot(&PathBuf::from(LEDGER_DB), epoch).unwrap();
        StakeDistributionSnapshot::new(&db).unwrap()
    }

    fn fixture_rewards_summary(snapshot: &StakeDistributionSnapshot) -> RewardsSummary {
        let db = RocksDB::from_snapshot(&PathBuf::from(LEDGER_DB), snapshot.epoch + 2).unwrap();
        RewardsSummary::new(&db, snapshot).unwrap()
    }

    fn compare_preprod_snapshot(epoch: Epoch) {
        let snapshot = fixture_stake_distribution_snapshot(epoch);
        insta::assert_json_snapshot!(format!("stake_distribution_{epoch}"), snapshot);
        let rewards_summary = fixture_rewards_summary(&snapshot);
        insta::assert_json_snapshot!(format!("rewards_summary_{epoch}"), rewards_summary);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_163() {
        compare_preprod_snapshot(163)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_164() {
        compare_preprod_snapshot(164)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_165() {
        compare_preprod_snapshot(165)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_166() {
        compare_preprod_snapshot(166)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_167() {
        compare_preprod_snapshot(167)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_168() {
        compare_preprod_snapshot(168)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_169() {
        compare_preprod_snapshot(169)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_170() {
        compare_preprod_snapshot(170)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_171() {
        compare_preprod_snapshot(171)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_172() {
        compare_preprod_snapshot(172)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_173() {
        compare_preprod_snapshot(173)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_174() {
        compare_preprod_snapshot(174)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_175() {
        compare_preprod_snapshot(175)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_176() {
        compare_preprod_snapshot(176)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_177() {
        compare_preprod_snapshot(177)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_178() {
        compare_preprod_snapshot(178)
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_179() {
        compare_preprod_snapshot(179)
    }
}
