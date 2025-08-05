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
    store::{columns::*, Snapshot, StoreError},
    summary::{
        governance::{DRepState, GovernanceSummary, ProposalState},
        safe_ratio,
        serde::{encode_drep, encode_pool_id, encode_stake_credential, serialize_map},
        AccountState, PoolState,
    },
};
use amaru_kernel::{
    expect_stake_credential, output_stake_credential, protocol_parameters::ProtocolParameters,
    DRep, HasLovelace, Lovelace, Network, PoolId, ProtocolVersion, StakeCredential,
    PROTOCOL_VERSION_10,
};
use iter_borrow::borrowable_proxy::BorrowableProxy;
use serde::ser::SerializeStruct;
use slot_arithmetic::Epoch;
use std::collections::{BTreeMap, BTreeSet};
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
        protocol_parameters: &ProtocolParameters,
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
            if let Some(credential) = output_stake_credential(&output) {
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
            .collect::<BTreeMap<PoolId, PoolState>>();

        let mut voting_stake: Lovelace = 0;
        let mut active_stake: Lovelace = 0;
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

                // NOTE: Only accounts delegated to active dreps counts towards the voting stake.
                if let Some(drep) = &account.drep {
                    if let Some(st) = dreps.get_mut(drep) {
                        voting_stake += account.lovelace + drep_deposits + refund;
                        st.stake += account.lovelace + drep_deposits + refund;
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
                                pool.voting_stake += pool_deposits;
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
            epoch = %epoch,
            accounts = %accounts.len(),
            pools = %pools.len(),
            active_stake = %active_stake,
            dreps = %dreps.len(),
            voting_stake = %voting_stake,
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
