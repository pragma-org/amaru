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

use crate::{
    store::{Snapshot, StoreError, columns::*},
    summary::{
        AccountState, PoolState,
        governance::{DRepState, GovernanceSummary},
        safe_ratio,
        serde::{encode_drep, encode_pool_id, encode_stake_credential, serialize_map},
    },
};
use amaru_iter_borrow::borrowable_proxy::BorrowableProxy;
use amaru_kernel::{
    DRep, Epoch, HasLovelace, Lovelace, Network, PoolId, ProtocolParameters, StakeCredential,
    expect_stake_credential,
};
use serde::ser::SerializeStruct;
use std::collections::BTreeMap;
use tracing::info;

const EVENT_TARGET: &str = "amaru::ledger::state::stake_distribution";

/// A stake distribution snapshot useful for:
///
/// - Leader schedule (in particular the 'pools' field)
/// - Rewards calculation
///
/// Note that the `accounts` field only contains _active_ accounts; that is, accounts
/// delegated to a registered stake pool.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct StakeDistribution {
    /// Epoch number for this snapshot (taken at the end of the epoch)
    pub epoch: Epoch,

    /// Total stake, in Lovelace, delegated to registered pools
    pub active_stake: Lovelace,

    /// Active stake plus deposits of ongoing proposals whose reward accounts are delegated to
    /// active stake pools.
    pub pools_voting_stake: Lovelace,

    /// Total voting stake, in Lovelace, corresponding to the total stake assigned to registered
    /// and active delegate representatives.
    pub dreps_voting_stake: Lovelace,

    /// Mapping of accounts' stake credentials to their respective state.
    ///
    /// Accounts that have stake but aren't delegated to any pools aren't present in the map.
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
        protocol_parameters: &ProtocolParameters,
        GovernanceSummary {
            mut dreps,
            deposits,
        }: GovernanceSummary,
    ) -> Result<Self, StoreError> {
        let epoch = db.epoch();

        let mut refunds = BTreeMap::new();
        let mut pools = db
            .iter_pools()?
            .map(|(pool, row)| {
                let reward_account = expect_stake_credential(&row.current_params.reward_account);

                // NOTE(POOL_VOTING_STAKE_DISTRIBUTION):
                //
                // We need to tick pool as part of the stake distribution calculation, in order to
                // know whether a pool will retire in the next epoch. This is because, votes
                // ratification happens *after* pools reaping, and thus, nullify voting power of
                // pools that are retiring.
                pools::Row::tick(
                    Box::new(BorrowableProxy::new(Some(row.clone()), |dropped| {
                        if dropped.is_none() {
                            // FIXME: Store the deposit with the pool, and ensures the same deposit
                            // it returned back.
                            //
                            // FIXME: Handle the case where there would be more than one refund (in
                            // case where many pools with a same reward account retire all at
                            // once).
                            refunds.insert(reward_account, protocol_parameters.stake_pool_deposit);
                        }
                    })),
                    epoch + 1,
                );

                (
                    pool,
                    PoolState {
                        registered_at: row.registered_at,
                        stake: 0,
                        voting_stake: 0,
                        blocks_count: 0,
                        // pre-compute margin here (1 - m), which gets used for all
                        // member and leader rewards calculation.
                        margin: safe_ratio(
                            row.current_params.margin.numerator,
                            row.current_params.margin.denominator,
                        ),
                        parameters: row.current_params,
                    },
                )
            })
            .collect::<BTreeMap<PoolId, PoolState>>();

        let mut accounts = db
            .iter_accounts()?
            .map(|(credential, account)| {
                (
                    credential,
                    AccountState {
                        lovelace: account.rewards,
                        pool: account.pool.and_then(|(pool, since)| {
                            let PoolState { registered_at, .. } = pools.get(&pool)?;
                            if &since >= registered_at {
                                Some(pool)
                            } else {
                                None
                            }
                        }),
                        drep: account.drep.and_then(|(drep, since)| match drep {
                            DRep::Abstain | DRep::NoConfidence => Some(drep),
                            DRep::Key { .. } | DRep::Script { .. } => {
                                let DRepState {
                                    previous_deregistration,
                                    ..
                                } = dreps.get(&drep)?;

                                // NOTE(PROTOCOL_VERSION_9):
                                //
                                // This is subtle. Delegation to a non-existing DRep was authorized
                                // in PROTOCOL_VERSION_9.
                                //
                                // It became correctly validated by ledger rules after.
                                //
                                // This means that there are cases where a delegation starts
                                // *before* a drep even existed. So we cannot simply check:
                                //
                                // 'if since > registered_at'
                                //
                                // It's also not correct to gate this condition by protocol version
                                // because invalid such delegation may pre-exist in the database,
                                // even when under PROTOCOL_VERSION_10.
                                //
                                // So we fallback to checking that no de-registration happened
                                // post-delegation.
                                if &Some(since) > previous_deregistration {
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
            if let Some(credential) = output.delegate() {
                let value = output.lovelace();
                accounts
                    .entry(credential)
                    .and_modify(|account| account.lovelace += value);
            }
        });

        let mut active_stake: Lovelace = 0;
        let mut pools_voting_stake: Lovelace = 0;
        let mut dreps_voting_stake: Lovelace = 0;

        let accounts = accounts
            .into_iter()
            .filter(|(credential, account)| {
                let (drep_deposits, pool_deposits) = deposits
                    .get(credential)
                    .map(|proposals| {
                        proposals
                            .iter()
                            .fold((0, 0), |(drep_deposits, pool_deposits), proposal| {
                                (
                                    drep_deposits + proposal.deposit,
                                    // NOTE(POOL_VOTING_STAKE_DISTRIBUTION):
                                    //
                                    // The pool distribution used for computing voting power is
                                    // determined BEFORE refunds or withdrawal are processed.
                                    //
                                    // So unlike the DRep voting stake, which already includes those,
                                    // we mustn't include the deposit as part of the pool voting stake
                                    // for the epoch that immediately follows the expiry.
                                    //
                                    // Note that the refund is eventually credited in the following
                                    // epoch so the deposit is effectively missing from the pools'
                                    // voting stake for an entire epoch.
                                    if epoch <= proposal.valid_until {
                                        pool_deposits + proposal.deposit
                                    } else {
                                        pool_deposits
                                    },
                                )
                            })
                    })
                    .unwrap_or((0, 0));

                let refund = refunds.get(credential).copied().unwrap_or_default();

                // Only accounts delegated to active dreps counts towards the voting stake.
                if let Some(drep) = &account.drep
                    && let Some(st) = dreps.get_mut(drep)
                {
                    // FIXME: DRep voting stake should also include:
                    //
                    // - refunds coming from *ratified* proposals, not only expired ones.
                    // - successful withdrawals ratified at the beginning of the ratification
                    //   epoch.
                    //
                    // The problem being that we cannot easily compute those from the current
                    // snapshot, since it requires either:
                    //
                    // - To replay ratification altogether.
                    // - Data from the future (i.e. the next snapshot).
                    dreps_voting_stake += account.lovelace + drep_deposits + refund;
                    st.stake += account.lovelace + drep_deposits + refund;
                }

                // Only accounts delegated to active pools counts towards the active stake.
                if let Some(pool_id) = account.pool {
                    return match pools.get_mut(&pool_id) {
                        None => false,
                        Some(pool) => {
                            // NOTE(POOL_VOTING_STAKE_DISTRIBUTION):
                            //
                            // Governance deposits do not count towards the pools' stake. They are
                            // only counted as part of the voting power.
                            let stake = account.lovelace;
                            active_stake += &stake;
                            pool.stake += &stake;

                            let voting_stake = stake + pool_deposits;
                            pool.voting_stake += &voting_stake;
                            pools_voting_stake += &voting_stake;

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
            epoch = %epoch,
            accounts = %accounts.len(),
            pools = %pools.len(),
            active_stake = %active_stake,
            dreps = %dreps.len(),
            dreps_voting_stake = %dreps_voting_stake,
            pools_voting_stake = %pools_voting_stake,
            "stake_distribution.snapshot",
        );

        Ok(StakeDistribution {
            epoch,
            active_stake,
            dreps_voting_stake,
            pools_voting_stake,
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
        let mut s = serializer.serialize_struct("StakeDistribution", 6)?;
        s.serialize_field("epoch", &self.0.epoch)?;
        s.serialize_field("active_stake", &self.0.active_stake)?;
        s.serialize_field("voting_stake", &self.0.dreps_voting_stake)?;
        serialize_map("accounts", &mut s, &self.0.accounts, |credential| {
            encode_stake_credential(self.1, credential)
        })?;
        serialize_map("pools", &mut s, &self.0.pools, encode_pool_id)?;
        serialize_map("dreps", &mut s, &self.0.dreps, encode_drep)?;
        s.end()
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::StakeDistribution;
    use crate::summary::{AccountState, PoolState, safe_ratio, stake_distribution::DRepState};
    use amaru_kernel::{
        Epoch, Lovelace, any_anchor, any_certificate_pointer, any_drep, any_hash28,
        any_pool_params, any_stake_credential, expect_stake_credential,
    };
    use proptest::{collection, option, prelude::*, prop_compose};
    use std::collections::BTreeMap;

    prop_compose! {
        pub fn any_stake_distribution_no_pools(
            min_epoch: u64,
            max_epoch: u64,
        )(
            epoch in any::<u64>(),
            active_stake_delta in any::<Lovelace>(),
            dreps in collection::btree_map(any_drep(), any_drep_state(min_epoch, max_epoch), 1..10),
            accounts in collection::btree_map(any_stake_credential(), any_account_state(), 1..20),
        ) -> StakeDistribution {
            let dreps_voting_stake = dreps.values().fold(0, |total, st| total + st.stake);

            let active_stake = if Lovelace::MAX - dreps_voting_stake >= active_stake_delta {
                Lovelace::MAX
            } else {
                dreps_voting_stake + active_stake_delta
            };

            StakeDistribution {
                epoch: Epoch::from(epoch),
                active_stake,
                dreps,
                dreps_voting_stake,
                accounts,
                pools: BTreeMap::new(),
                pools_voting_stake: 0,
            }
        }
    }

    prop_compose! {
        pub fn any_stake_distribution_no_dreps()(
            epoch in any::<u64>(),
            pools in collection::btree_map(any_hash28(), any_pool_state(), 1..10),
            accounts in collection::btree_map(any_stake_credential(), any_account_state(), 1..20),
        ) -> StakeDistribution {
            let active_stake = pools.values().fold(0, |total, st| total + st.stake);
            let pools_voting_stake = pools.values().fold(0, |total, st| total + st.voting_stake);

            let pools_len = pools.len();

            let pools_vec = pools.iter().collect::<Vec<_>>();

            // Artificially create some links between pools and accounts.
            let accounts = accounts.into_iter().enumerate().map(|(ix, (mut account, mut account_st))| {
                let (pool, pool_st) =
                    pools_vec
                        .get(ix % pools_len)
                        .unwrap_or_else(|| unreachable!("% pools_len guarantees it's some"));

                // Ensure some of the reward accounts do exists.
                if ix % 2 == 0 {
                        account = expect_stake_credential(&pool_st.parameters.reward_account);
                }

                // Make sure accounts are delegated to existing pools, when they are.
                if let Some(delegation) = account_st.pool.as_mut() {
                    *delegation = **pool;
                }

                (account, account_st)
            }).collect();

            StakeDistribution {
                epoch: Epoch::from(epoch),
                active_stake,
                pools,
                pools_voting_stake,
                accounts,
                dreps: BTreeMap::new(),
                dreps_voting_stake: 0,
            }
        }
    }

    prop_compose! {
        pub fn any_account_state()(
            lovelace in any::<Lovelace>(),
            pool in option::of(any_hash28()),
            drep in option::of(any_drep()),
        ) -> AccountState {
            AccountState {
                lovelace,
                pool,
                drep
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
            stake in 0_u64..1_000_000_000_000,
            registered_at in any_certificate_pointer(u64::MAX),
            previous_deregistration in option::of(any_certificate_pointer(u64::MAX)),
        ) -> DRepState {
            // Ensure registered at is always strictly after previous de-registrations.
            let (registered_at, previous_deregistration) = if previous_deregistration > Some(registered_at) {
                #[expect(clippy::unwrap_used)]
                // NOTE: .unwrap can't fail because of the 'if' guard.
                (previous_deregistration.unwrap(), Some(registered_at))
            } else if previous_deregistration == Some(registered_at) {
                (registered_at, None)
            } else {
                (registered_at, previous_deregistration)
            };


            DRepState {
                valid_until: Some(Epoch::from(valid_until)),
                metadata,
                stake,
                registered_at,
                previous_deregistration
            }
        }
    }
}
