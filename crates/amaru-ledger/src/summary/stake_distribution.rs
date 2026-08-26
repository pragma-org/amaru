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

use std::{
    collections::BTreeMap,
    ops::Deref,
    sync::{OnceLock, atomic, atomic::AtomicUsize},
};

use amaru_kernel::{Credential, DRep, Epoch, Hash, Lovelace, NetworkName, PoolId, SortedPairs, safe_ratio};
use amaru_observability::info;
use serde::ser::SerializeStruct;

use crate::{
    epoch_transition::PoolsEpochTransitionUpdates,
    store::{Snapshot, StoreError, columns::pots::Row as Pots},
    summary::{
        AccountState, PoolState,
        governance::{DRepState, GovernanceSummary},
        serde::serialize_map,
    },
};

/// A stake summary snapshot useful for:
///
/// - Leader schedule (in particular the 'pools' field)
/// - Rewards calculation
///
/// Note that the `accounts` field only contains _active_ accounts; that is, accounts
/// delegated to a registered stake pool.
///
/// Fields are public and individually clonable (`AccountState`, `PoolState`,
/// `DRepState`, scalars, map entries). The aggregate intentionally does **not**
/// implement [`Clone`] so large copies require explicit field-level intent.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct StakeSummary {
    /// The epoch stake distribution and other related stake information
    pub stake_distribution: StakeDistribution,

    /// Mapping of accounts' stake credentials to their respective state.
    ///
    /// Accounts that have stake but aren't delegated to any pools aren't present in the map.
    pub accounts: SortedPairs<Credential, AccountState>,
}

impl Deref for StakeSummary {
    type Target = StakeDistribution;
    fn deref(&self) -> &Self::Target {
        &self.stake_distribution
    }
}

/// A slim stake distribution retained in-memory by the ledger runtime.
///
/// Unlike [`StakeSummary`], this deliberately omits the full accounts mapping. The only
/// account-derived information needed on the hot path is captured in `PoolState::fallback_drep`.
///
/// Does not implement [`Clone`] in non-test builds: clone pools/dreps/scalars individually
/// when an embedder needs independent ownership.
#[derive(Debug, Default)]
#[cfg_attr(test, derive(Clone))]
pub struct StakeDistribution {
    /// Epoch number for this snapshot (taken at the end of the epoch)
    pub epoch: Epoch,

    /// Treasury value for the matching epoch
    pub treasury: Lovelace,

    /// Reserves value for the matching epoch
    pub reserves: Lovelace,

    /// Total stake, in Lovelace, delegated to registered pools
    pub active_stake: Lovelace,

    /// Active stake plus deposits of ongoing proposals whose reward accounts are delegated to
    /// active stake pools.
    pub pools_voting_stake: Lovelace,

    /// Total voting stake, in Lovelace, corresponding to the total stake assigned to registered
    /// and active delegate representatives.
    pub dreps_voting_stake: Lovelace,

    /// Mapping of pools to their relative stake & parameters
    pub pools: BTreeMap<PoolId, PoolState>,

    /// Mapping of dreps to their relative stake
    pub dreps: BTreeMap<DRep, DRepState>,
}

const PROGRESS_BATCH_SIZE: usize = 1_000;

impl StakeSummary {
    /// Compute a new stake summary snapshot using data available in the `Store`.
    ///
    /// Invariant: The given store is expected to be a snapshot taken at the end of an epoch.
    pub fn new(
        db: &impl Snapshot,
        GovernanceSummary { mut dreps, pools_deposits, dreps_deposits }: GovernanceSummary,
        network: NetworkName,
        mut notify: impl FnMut(f64),
    ) -> Result<Self, StoreError> {
        let epoch = db.epoch();
        let stake_pool_deposit = db.protocol_parameters()?.stake_pool_deposit;
        let mut pools_deregistration_refunds: BTreeMap<Credential, Lovelace> = BTreeMap::new();

        let mut pools = db
            .iter_pools()?
            .map(|(pool, row)| {
                // NOTE: Pool voting stake distribution & pool retirements
                //
                // We need to tick pool as part of the stake distribution calculation, in order to
                // know whether a pool will retire in the next epoch. This is because, votes
                // ratification happens *after* pools reaping, and thus, nullify voting power of
                // pools that are retiring.
                if PoolsEpochTransitionUpdates::is_retiring(epoch + 1, &row) {
                    let reward_account = row.current_params.reward_account.credential();
                    pools_deregistration_refunds
                        .entry(reward_account)
                        .and_modify(|refund| *refund += stake_pool_deposit)
                        .or_insert(stake_pool_deposit);
                }

                (
                    pool,
                    PoolState {
                        registered_at: row.registered_at,
                        blocks_count: 0,
                        stake: 0,
                        voting_stake: 0,
                        margin: safe_ratio(row.current_params.margin.numerator, row.current_params.margin.denominator),
                        parameters: row.current_params,
                        fallback_drep: None,
                    },
                )
            })
            .collect::<BTreeMap<PoolId, PoolState>>();

        let mut accounts = Vec::with_capacity(Capacity::get_or_init(db));

        for (credential, row) in db.iter_accounts()? {
            let state = AccountState {
                balance: row.rewards,
                rewards: 0,
                pool: row.pool.and_then(|(pool, since)| {
                    let PoolState { registered_at, .. } = pools.get(&pool)?;
                    if &since >= registered_at { Some(pool) } else { None }
                }),
                drep: row.drep.and_then(|(drep, since)| match drep {
                    DRep::Abstain | DRep::NoConfidence => Some(drep),
                    DRep::Key { .. } | DRep::Script { .. } => {
                        let DRepState { registered_at, .. } = dreps.get(&drep)?;
                        if &since >= registered_at { Some(drep) } else { None }
                    }
                }),
            };

            accounts.push((credential, state));
        }

        // NOTE: discrepancy between Ord and serialised ordering
        //
        // Weirdly enough, the variants of a Credential comes with the script hash first,
        // and then the key hash. However, they are serialised the other away around (key
        // variant comes with tag index 0, whereas script is 1).
        //
        // RocksDB yields data in ascending order of the serialised key; so we append them in
        // db-order, and turn them into a sorted vector afterwards by sorting in-place.
        //
        // This trick allows to allocate only a single vector with minimal overhead.
        let mut accounts = SortedPairs::from(accounts);

        let accounts_len = accounts.len();
        Capacity::update(accounts_len);

        let total_work = network.estimated_utxo_size().saturating_add(accounts_len.saturating_mul(2)).max(1);
        let progress_after_accounts = (accounts_len as f64 / total_work as f64).clamp(0.0, 1.0);
        notify(progress_after_accounts);

        let mut processed_utxos = 0_usize;
        let mut last_reported_utxos = 0_usize;
        let mut notify_progress = |processed_utxos: usize| {
            let progress =
                ((accounts_len.saturating_add(processed_utxos)) as f64 / total_work as f64).clamp(0.0, 0.999);
            notify(progress);
        };

        db.iter_stake_distribution()?.for_each(|stake| {
            if let Some(credential) = stake.credential
                && let Some(account) = accounts.get_mut(&credential)
            {
                account.balance += stake.lovelace;
            }

            processed_utxos += 1;

            if processed_utxos.saturating_sub(last_reported_utxos) >= PROGRESS_BATCH_SIZE {
                last_reported_utxos = processed_utxos;
                notify_progress(processed_utxos);
            }
        });

        if processed_utxos != last_reported_utxos {
            notify_progress(processed_utxos);
        }

        let mut active_stake: Lovelace = 0;
        let mut pools_voting_stake: Lovelace = 0;
        let mut dreps_voting_stake: Lovelace = 0;

        for (credential, account) in accounts.iter() {
            // Only accounts delegated to active dreps counts towards the voting stake.
            if let Some(drep) = &account.drep
                && let Some(st) = dreps.get_mut(drep)
            {
                let deregistration_refunds = pools_deregistration_refunds.get(credential).copied().unwrap_or_default();
                let proposal_deposits_and_withdrawals = dreps_deposits.get(credential).copied().unwrap_or_default();
                let voting_stake = account.balance + proposal_deposits_and_withdrawals + deregistration_refunds;
                dreps_voting_stake += &voting_stake;
                st.voting_stake += &voting_stake;
            }

            // Only accounts delegated to active pools counts towards the active stake.
            if let Some(pool_id) = account.pool
                && let Some(pool) = pools.get_mut(&pool_id)
            {
                let proposal_deposits = pools_deposits.get(credential).copied().unwrap_or_default();
                let stake = account.balance;
                active_stake += &stake;
                pool.stake += &stake;

                let voting_stake = stake + proposal_deposits;
                pool.voting_stake += &voting_stake;
                pools_voting_stake += &voting_stake;
            }
        }

        for pool in pools.values_mut() {
            let reward_account = pool.parameters.reward_account.credential();
            pool.fallback_drep = accounts.get(&reward_account).and_then(|account| account.drep);
        }

        db.iter_block_issuers()?.for_each(|(_, issuer)| {
            pools.entry(issuer.slot_leader).and_modify(|pool| pool.blocks_count += 1);
        });

        let Pots { reserves, treasury, .. } = db.pots()?;

        notify_progress(total_work);

        info!(
            ledger::stake_distribution::SNAPSHOT,
            accounts = accounts.len(),
            dreps = dreps.len(),
            pools = pools.len(),
            active_stake,
            pools_voting_stake,
            dreps_voting_stake,
        );

        Ok(Self {
            stake_distribution: StakeDistribution {
                epoch,
                treasury,
                reserves,
                active_stake,
                pools_voting_stake,
                dreps_voting_stake,
                pools,
                dreps,
            },
            accounts,
        })
    }
}

impl serde::Serialize for StakeSummary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("StakeSummary", 9)?;

        s.serialize_field("epoch", &self.epoch)?;
        s.serialize_field("treasury", &self.treasury)?;
        s.serialize_field("reserves", &self.reserves)?;
        s.serialize_field("active_stake", &self.active_stake)?;
        s.serialize_field("dreps_voting_stake", &self.dreps_voting_stake)?;
        s.serialize_field("pools_voting_stake", &self.pools_voting_stake)?;

        #[derive(serde::Serialize, Default)]
        struct Accounts<'a> {
            scripts: BTreeMap<Hash<28>, &'a AccountState>,
            verification_keys: BTreeMap<Hash<28>, &'a AccountState>,
        }
        s.serialize_field(
            "accounts",
            &self.accounts.iter().fold(Accounts::default(), |mut accounts, (credential, st)| {
                match credential {
                    Credential::KeyHash(hash) => accounts.verification_keys.insert(*hash, st),
                    Credential::ScriptHash(hash) => accounts.scripts.insert(*hash, st),
                };

                accounts
            }),
        )?;

        #[derive(serde::Serialize, Default)]
        struct DReps<'a> {
            abstain: VotingStake,
            no_confidence: VotingStake,
            scripts: BTreeMap<Hash<28>, &'a DRepState>,
            verification_keys: BTreeMap<Hash<28>, &'a DRepState>,
        }
        #[derive(serde::Serialize, Default)]
        struct VotingStake {
            voting_stake: Lovelace,
        }
        s.serialize_field(
            "dreps",
            &self.dreps.iter().fold(DReps::default(), |mut dreps, (drep, st)| {
                match drep {
                    DRep::Abstain => {
                        dreps.abstain = VotingStake { voting_stake: st.voting_stake };
                    }
                    DRep::NoConfidence => {
                        dreps.no_confidence = VotingStake { voting_stake: st.voting_stake };
                    }
                    DRep::Script(hash) => {
                        dreps.scripts.insert(*hash, st);
                    }
                    DRep::Key(hash) => {
                        dreps.verification_keys.insert(*hash, st);
                    }
                };

                dreps
            }),
        )?;

        serialize_map("pools", &mut s, &self.pools, |id| hex::encode(id))?;

        s.end()
    }
}

/// A type to inform of the ideal capacity for accounts in rewards.
#[derive(Debug, Default, Clone, Copy)]
struct Capacity<T>(T);

static CAPACITY: OnceLock<Capacity<AtomicUsize>> = OnceLock::new();

impl Capacity<usize> {
    /// Record updated lengths as hints for the next query
    fn update(len: usize) {
        if let Some(capacity) = OnceLock::get(&CAPACITY) {
            capacity.0.store(len, atomic::Ordering::Relaxed);
        }
    }

    /// Get the value of the current capacity, or resolve it once from the database.
    #[expect(clippy::panic)]
    fn get_or_init(db: &impl Snapshot) -> usize {
        let base_capacity = CAPACITY.get_or_init(|| {
            Capacity(AtomicUsize::new(
                db.iter_accounts().unwrap_or_else(|e| panic!("unable to initialize accounts capacities: {e}")).count(),
            ))
        });

        let len = base_capacity.0.load(atomic::Ordering::Relaxed);

        // We always return a little more than what's needed to cope with new accounts
        // registrations and with unclaimed rewards.
        len + len / 10
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{
        Epoch, Lovelace, any_anchor, any_certificate_pointer, any_credential, any_drep, any_hash28, any_pool_params,
        safe_ratio,
    };
    use proptest::{collection, option, prelude::*, prop_compose};

    use super::StakeDistribution;
    use crate::summary::{AccountState, PoolState, stake_distribution::DRepState};

    prop_compose! {
        pub fn any_stake_distribution_no_pools(
            min_epoch: u64,
            max_epoch: u64,
        )(
            epoch in any::<u64>(),
            treasury in any::<u64>(),
            reserves in any::<u64>(),
            active_stake_delta in any::<Lovelace>(),
            dreps in collection::btree_map(any_drep(), any_drep_state(min_epoch, max_epoch), 1..10),
            _accounts in collection::btree_map(any_credential(), any_account_state(), 1..20),
        ) -> StakeDistribution {
            let dreps_voting_stake = dreps.values().fold(0, |total, st| total + st.voting_stake);

            let active_stake =
                if Lovelace::MAX - dreps_voting_stake >= active_stake_delta { Lovelace::MAX } else { dreps_voting_stake + active_stake_delta };

            StakeDistribution {
                epoch: Epoch::from(epoch),
                treasury,
                reserves,
                active_stake,
                dreps,
                dreps_voting_stake,
                pools: BTreeMap::new(),
                pools_voting_stake: 0,
            }
        }
    }

    prop_compose! {
        pub fn any_stake_distribution_no_dreps()(
            epoch in any::<u64>(),
            treasury in any::<u64>(),
            reserves in any::<u64>(),
            pools in collection::btree_map(any_hash28(), any_pool_state(), 1..10),
            accounts in collection::btree_map(any_credential(), any_account_state(), 1..20),
        ) -> StakeDistribution {
            let active_stake = pools.values().fold(0, |total, st| total + st.stake);
            let pools_voting_stake = pools.values().fold(0, |total, st| total + st.voting_stake);

            let pools_len = pools.len();

            let pools_vec = pools.iter().collect::<Vec<_>>();

            // Artificially create some links between pools and accounts.
            let accounts = accounts
                .into_iter()
                .enumerate()
                .map(|(ix, (mut account, mut account_st))| {
                    let (pool, pool_st) = pools_vec
                        .get(ix % pools_len)
                        .unwrap_or_else(|| unreachable!("% pools_len guarantees it's some"));

                    // Ensure some of the reward accounts do exists.
                    if ix % 2 == 0 {
                        account = pool_st.parameters.reward_account.credential();
                    }

                    // Make sure accounts are delegated to existing pools, when they are.
                    if let Some(delegation) = account_st.pool.as_mut() {
                        *delegation = **pool;
                    }

                    (account, account_st)
                })
                .collect::<BTreeMap<_, _>>();

            let mut pools = pools;

            for pool in pools.values_mut() {
                let reward_account = pool.parameters.reward_account.credential();
                pool.fallback_drep = accounts.get(&reward_account).and_then(|account| account.drep);
            }

            StakeDistribution {
                epoch: Epoch::from(epoch),
                treasury,
                reserves,
                active_stake,
                pools,
                pools_voting_stake,
                dreps: BTreeMap::new(),
                dreps_voting_stake: 0,
            }
        }
    }

    prop_compose! {
        pub fn any_account_state()(
            balance in any::<Lovelace>(),
            pool in option::of(any_hash28()),
            drep in option::of(any_drep()),
        ) -> AccountState {
            AccountState {
                balance,
                pool,
                drep,
                rewards: 0,
            }
        }
    }

    prop_compose! {
        pub fn any_pool_state()(
            registered_at in any_certificate_pointer(u64::MAX),
            blocks_count in any::<u64>(),
            stake in 0_u64..1_000_000_000_000,
            voting_stake in 0_u64..1_000_000_000_000,
            parameters in any_pool_params(),
            fallback_drep in option::of(any_drep()),
        ) -> PoolState {
            let margin = safe_ratio(
                parameters.margin.numerator,
                parameters.margin.denominator,
            );

            PoolState {
                registered_at,
                blocks_count,
                stake,
                voting_stake: stake.max(voting_stake),
                margin,
                parameters,
                fallback_drep,
            }
        }
    }

    prop_compose! {
        pub fn any_drep_state(
            min_epoch: u64,
            max_epoch: u64,
        )(
            valid_until in min_epoch..=max_epoch,
            metadata in option::of(any_anchor()),
            voting_stake in 0_u64..1_000_000_000_000,
            registered_at in any_certificate_pointer(u64::MAX),
        ) -> DRepState {
            DRepState {
                valid_until: Some(Epoch::from(valid_until)),
                metadata: metadata.map(Box::new),
                voting_stake,
                registered_at,
            }
        }
    }
}
