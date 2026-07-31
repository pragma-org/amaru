// Copyright 2025 PRAGMA
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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    Certificate, CertificatePointer, DRep, DRepRegistration, Epoch, EraHistory, EraHistoryError, Hash, Lovelace,
    MemoizedDatum, Network, NonEmptySet, PoolId, PoolParams, ProtocolParameters, RedeemerTag, RequiredScript,
    StakeCredential, TransactionPointer, parse_reward_account, size::SCRIPT,
};
use thiserror::Error;

use crate::{
    context::{
        AccountState, AccountsSlice, BalanceSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError, PoolsSlice,
        RegisterError, UnregisterError, UpdateError, WitnessSlice,
    },
    epoch_transition::GovernanceActivity,
};

#[derive(Debug, Error)]
pub enum InvalidCertificates {
    #[error("stake credential already registered: {0}")]
    StakeCredentialAlreadyRegistered(#[from] RegisterError<AccountState, StakeCredential>),

    #[error("invalid stake credential pool delegation: {0}")]
    StakeCredentialInvalidPoolDelegation(#[from] DelegateError<StakeCredential, PoolId>),

    #[error("invalid stake credential vote delegation: {0}")]
    StakeCredentialInvalidVoteDelegation(#[from] DelegateError<StakeCredential, DRep>),

    #[error("drep already registered: {0}")]
    DRepAlreadyRegistered(#[from] RegisterError<DRepRegistration, StakeCredential>),

    #[error("invalid drep attempted update: {0}")]
    DRepInvalidUpdate(#[from] UpdateError<StakeCredential>),

    #[error("unknown cc member: {0}")]
    CCMemberUnknown(#[from] UnregisterError<CCMember, StakeCredential>),

    #[error("invalid cc member hot credential delegation: {0}")]
    CCMemberInvalidDelegation(#[from] DelegateError<StakeCredential, StakeCredential>),

    #[error("impossible slot arithmetic: {0}")]
    ImpossibleSlotArithmetic(#[from] EraHistoryError),

    #[error("pool reward account has wrong network: expected {expected:?}, actual {actual:?}")]
    PoolWrongNetwork { expected: Network, actual: Network },

    #[error("pool reward account is malformed")]
    PoolMalformedRewardAccount,

    #[error("pool cost too low: provided {provided}, minimum {minimum}")]
    PoolCostTooLow { provided: Lovelace, minimum: Lovelace },

    #[error("pool retirement epoch out of range: epoch {epoch}, must satisfy {current_epoch} < epoch <= {max_epoch}")]
    PoolRetirementWrongEpoch { epoch: Epoch, current_epoch: Epoch, max_epoch: Epoch },

    #[error("unknown pool: {0}")]
    StakePoolUnknown(#[from] UnregisterError<PoolId, PoolId>),

    #[error("incorrect stake deposit: provided {provided}, expected {expected}")]
    IncorrectStakeDeposit { provided: Lovelace, expected: Lovelace },

    #[error("incorrect drep deposit: provided {provided}, expected {expected}")]
    IncorrectDRepDeposit { provided: Lovelace, expected: Lovelace },

    #[error("stake credential not registered: {0:?}")]
    StakeCredentialNotRegistered(StakeCredential),

    #[error("cannot unregister a stake credential that has rewards: {credential:?} has a balance of {rewards}")]
    StakeCredentialHasRewards { credential: StakeCredential, rewards: Lovelace },

    #[error("drep not registered: {0:?}")]
    DRepNotRegistered(StakeCredential),
}

pub(crate) fn execute<C>(
    context: &mut C,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: GovernanceActivity,
    transaction: TransactionPointer,
    certificates: Option<NonEmptySet<Certificate>>,
) -> Result<(), InvalidCertificates>
where
    C: PoolsSlice + AccountsSlice + DRepsSlice + CommitteeSlice + WitnessSlice + BalanceSlice,
{
    certificates.map(|xs| xs.to_vec()).unwrap_or_default().into_iter().enumerate().try_for_each(
        |(certificate_index, certificate)| {
            execute_one(
                context,
                network,
                protocol_parameters,
                era_history,
                governance_activity,
                CertificatePointer { transaction, certificate_index },
                certificate,
            )
        },
    )
}

/// A simplified version of `execute` which does not validate anything, but count deposits and
/// refunds.
pub(crate) fn count_lovelace<C>(
    context: &mut C,
    protocol_parameters: &ProtocolParameters,
    certificates: Option<NonEmptySet<Certificate>>,
) where
    C: PoolsSlice + AccountsSlice + DRepsSlice + BalanceSlice,
{
    let mut pools = BTreeSet::new();
    let mut accounts = BTreeMap::new();
    let mut dreps = BTreeMap::new();

    let mut delta: i64 = 0;
    for certificate in certificates.map(|xs| xs.to_vec()).unwrap_or_default().into_iter() {
        delta += count_lovelace_one(context, protocol_parameters, &mut pools, &mut accounts, &mut dreps, certificate);
    }

    if delta > 0 { context.produce_lovelace(delta as u64) } else { context.consume_lovelace(delta as u64) }
}

// FIXME: Perform all necessary rules validations down here.
fn execute_one<C>(
    context: &mut C,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: GovernanceActivity,
    pointer: CertificatePointer,
    certificate: Certificate,
) -> Result<(), InvalidCertificates>
where
    C: PoolsSlice + AccountsSlice + DRepsSlice + CommitteeSlice + WitnessSlice + BalanceSlice,
{
    // Promote a ScriptHash into a RequiredScript, with additional context needed to defer the
    // validation of the script.
    let into_required_script = |hash: Hash<SCRIPT>| -> RequiredScript {
        RequiredScript {
            hash,
            index: pointer.certificate_index as u32,
            purpose: RedeemerTag::Cert,
            datum: MemoizedDatum::None,
        }
    };

    match certificate {
        Certificate::PoolRegistration(params) => {
            let PoolParams { id, cost, reward_account, owners, .. } = params.as_ref();

            context.require_vkey_witness(*id);

            // https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/shelley/impl/src/Cardano/Ledger/Shelley/UTxO.hs#L250-L256
            // The Haskell node requires both the owners and the operators, which may be the same pkh.
            // TODO: We need coverage for this branch, we have none in either conformance tests or unit tests.
            for owner in owners.iter() {
                context.require_vkey_witness(*owner);
            }

            let reward_account_network =
                parse_reward_account(reward_account).ok_or(InvalidCertificates::PoolMalformedRewardAccount)?.1;

            if reward_account_network != network {
                return Err(InvalidCertificates::PoolWrongNetwork {
                    expected: network,
                    actual: reward_account_network,
                });
            }

            if cost < &protocol_parameters.min_pool_cost {
                return Err(InvalidCertificates::PoolCostTooLow {
                    provided: *cost,
                    minimum: protocol_parameters.min_pool_cost,
                });
            }

            // TODO: Have `register` return this information
            let is_new_pool = !context.exists(*id);

            PoolsSlice::register(context, *params, pointer, protocol_parameters.stake_pool_deposit);

            if is_new_pool {
                context.produce_lovelace(protocol_parameters.stake_pool_deposit);
            }

            Ok(())
        }

        Certificate::PoolRetirement(id, retirement_epoch) => {
            context.require_vkey_witness(id);

            // NOTE: Some conformance tests fail this check because the Haskell imp tests run on
            // a synthetic test chain whose epoch/slot mapping differs from our era_history. Our
            // slot_to_epoch computes a different current epoch, making the range check reject
            // transactions that the Haskell node accepts.
            let current_epoch = era_history.slot_to_epoch_unchecked_horizon(pointer.slot())?;
            let max_epoch = current_epoch + protocol_parameters.stake_pool_max_retirement_epoch;
            if retirement_epoch <= current_epoch || retirement_epoch > max_epoch {
                return Err(InvalidCertificates::PoolRetirementWrongEpoch {
                    epoch: retirement_epoch,
                    current_epoch,
                    max_epoch,
                });
            }

            PoolsSlice::retire(context, id, retirement_epoch)?;

            Ok(())
        }

        Certificate::StakeRegistration(credential) => {
            AccountsSlice::register(
                context,
                credential,
                AccountState {
                    deposit: protocol_parameters.stake_credential_deposit,
                    pool: None,
                    drep: None,
                    rewards: 0,
                },
            )?;

            context.produce_lovelace(protocol_parameters.stake_credential_deposit);

            Ok(())
        }

        Certificate::Reg(credential, deposit) => {
            // The "old behavior of not requiring a witness for staking credential registration" is mantained:
            // - Only during the "transitional period of Conway"
            // - Only for staking credential registration certificates without a deposit
            //
            // See https://github.com/IntersectMBO/cardano-ledger/blob/81637a1c2250225fef47399dd56f80d87384df32/eras/conway/impl/src/Cardano/Ledger/Conway/TxCert.hs#L698
            if deposit > 0 {
                match credential {
                    StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                    StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
                };
            }

            let expected = protocol_parameters.stake_credential_deposit;
            if deposit != expected {
                return Err(InvalidCertificates::IncorrectStakeDeposit { provided: deposit, expected });
            }

            AccountsSlice::register(context, credential, AccountState { deposit, pool: None, drep: None, rewards: 0 })?;
            context.produce_lovelace(deposit);

            Ok(())
        }

        Certificate::StakeDeregistration(credential) => {
            match credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            let account = AccountsSlice::lookup(context, &credential)
                .ok_or(InvalidCertificates::StakeCredentialNotRegistered(credential))?;

            if account.rewards != 0 {
                return Err(InvalidCertificates::StakeCredentialHasRewards { credential, rewards: account.rewards });
            }

            AccountsSlice::unregister(context, credential);
            context.consume_lovelace(account.deposit);

            Ok(())
        }

        Certificate::UnReg(credential, refund) => {
            match credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            let account = AccountsSlice::lookup(context, &credential)
                .ok_or(InvalidCertificates::StakeCredentialNotRegistered(credential))?;

            if refund != account.deposit {
                return Err(InvalidCertificates::IncorrectStakeDeposit { provided: refund, expected: account.deposit });
            }

            if account.rewards != 0 {
                return Err(InvalidCertificates::StakeCredentialHasRewards { credential, rewards: account.rewards });
            }

            AccountsSlice::unregister(context, credential);
            context.consume_lovelace(refund);

            Ok(())
        }

        Certificate::StakeDelegation(credential, pool) => {
            match credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            context.delegate_pool(credential, pool, pointer)?;

            Ok(())
        }

        Certificate::RegDRepCert(drep, deposit, anchor) => {
            match drep {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            let expected = protocol_parameters.drep_deposit;
            if deposit != expected {
                return Err(InvalidCertificates::IncorrectDRepDeposit { provided: deposit, expected });
            }

            let valid_until = era_history.slot_to_epoch_unchecked_horizon(pointer.slot())?
                + protocol_parameters.drep_expiry
                - governance_activity.consecutive_dormant_epochs as u64;

            DRepsSlice::register(
                context,
                drep,
                DRepRegistration { deposit, registered_at: pointer, valid_until },
                anchor,
            )?;

            context.produce_lovelace(deposit);

            Ok(())
        }

        Certificate::UnRegDRepCert(drep, refund) => {
            match drep {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            let deposit = match DRepsSlice::lookup(context, &drep) {
                Some(registration) => registration.deposit,
                None => return Err(InvalidCertificates::DRepNotRegistered(drep)),
            };

            if refund != deposit {
                return Err(InvalidCertificates::IncorrectDRepDeposit { provided: refund, expected: deposit });
            }

            DRepsSlice::unregister(context, drep, refund, pointer);
            context.consume_lovelace(refund);

            Ok(())
        }

        Certificate::UpdateDRepCert(drep, anchor) => {
            match drep {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            DRepsSlice::update(context, drep, anchor)?;

            Ok(())
        }

        Certificate::VoteDeleg(credential, drep) => {
            match credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };

            AccountsSlice::delegate_vote(context, credential, drep, pointer)?;

            Ok(())
        }

        Certificate::AuthCommitteeHot(cold_credential, hot_credential) => {
            match cold_credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };
            CommitteeSlice::delegate_cold_key(context, cold_credential, hot_credential)?;
            Ok(())
        }

        Certificate::ResignCommitteeCold(cold_credential, anchor) => {
            match cold_credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };
            CommitteeSlice::resign(context, cold_credential, anchor)?;
            Ok(())
        }

        Certificate::StakeVoteDeleg(credential, pool, drep) => {
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, drep_deleg)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, pool_deleg)
        }

        Certificate::StakeRegDeleg(credential, pool, coin) => {
            let reg = Certificate::Reg(credential, coin);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, reg)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, pool_deleg)
        }

        Certificate::StakeVoteRegDeleg(credential, pool, drep, coin) => {
            let reg = Certificate::Reg(credential, coin);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, reg)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, pool_deleg)?;
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, drep_deleg)
        }

        Certificate::VoteRegDeleg(credential, drep, coin) => {
            let reg = Certificate::Reg(credential, coin);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, reg)?;
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, drep_deleg)
        }
    }
}

// NOTE: The 'deposit' value inside certificates is not used here;
//
// since it will not be validated, we must use instead the value from the protocol
// parameters.
fn count_lovelace_one<C>(
    context: &mut C,
    protocol_parameters: &ProtocolParameters,
    local_pools_slice: &mut BTreeSet<PoolId>,
    local_accounts_slice: &mut BTreeMap<StakeCredential, Lovelace>,
    local_dreps_slice: &mut BTreeMap<StakeCredential, Lovelace>,
    certificate: Certificate,
) -> i64
where
    C: PoolsSlice + AccountsSlice + DRepsSlice + BalanceSlice,
{
    use Certificate::*;

    match certificate {
        PoolRegistration(params) => {
            // TODO: Have tests covering local state changes in certificate accounting
            //
            // See note below.
            if !local_pools_slice.contains(&params.id) && !context.exists(params.id) {
                local_pools_slice.insert(params.id);
                protocol_parameters.stake_pool_deposit as i64
            } else {
                0
            }
        }

        Reg(credential, ..)
        | StakeRegistration(credential, ..)
        | StakeRegDeleg(credential, ..)
        | StakeVoteRegDeleg(credential, ..)
        | VoteRegDeleg(credential, ..) => {
            let deposit = protocol_parameters.stake_credential_deposit;
            local_accounts_slice.insert(credential, deposit);
            deposit as i64
        }

        StakeDeregistration(credential) | UnReg(credential, ..) => {
            // TODO: Have tests covering local state changes in certificate accounting
            //
            // This is a subtle but, when counting lovelace for withdrawals, we don't modify the
            // state directly, and so, we must manually account for deposits and refunds
            // interactions within the transaction itself.
            //
            // Suppose for example that we have an account A registered with a deposit D1; and a
            // transaction containing a de-registration for A, and a re-registration with deposit
            // D2, and a de-registration again.
            //
            // In this scenario, we must count a refund for D1, and then for D2 which may be two
            // different values. So we use these local slices to remember the local changes. Note
            // that this is fully local to the transaction since we only use this accounting for
            // collaterals.
            let deposit = local_accounts_slice
                .remove(&credential)
                .or_else(|| AccountsSlice::lookup(context, &credential).map(|registration| registration.deposit))
                .unwrap_or_default() as i64;
            -deposit
        }

        RegDRepCert(credential, ..) => {
            let deposit = protocol_parameters.drep_deposit;
            local_dreps_slice.insert(credential, deposit);
            deposit as i64
        }

        UnRegDRepCert(credential, ..) => {
            // TODO: Have tests covering local stte changes in certificate accounting
            //
            // See note above.
            let deposit = local_dreps_slice
                .remove(&credential)
                .or_else(|| DRepsSlice::lookup(context, &credential).map(|registration| registration.deposit))
                .unwrap_or_default() as i64;
            -deposit
        }

        PoolRetirement(..)
        | StakeDelegation(..)
        | UpdateDRepCert(..)
        | VoteDeleg(..)
        | AuthCommitteeHot(..)
        | ResignCommitteeCold(..)
        | StakeVoteDeleg(..) => 0,
    }
}
