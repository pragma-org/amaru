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

use amaru_kernel::{
    Certificate, CertificatePointer, DRep, DRepRegistration, Epoch, EraHistory, EraHistoryError, Hash, Lovelace,
    MemoizedDatum, Network, NonEmptySet, PROTOCOL_VERSION_9, PoolId, PoolParams, ProtocolParameters, RequiredScript,
    ScriptPurpose, StakeCredential, TransactionPointer, parse_reward_account, size::SCRIPT,
};
use thiserror::Error;

use crate::{
    context::{
        AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError, PoolsSlice, RegisterError,
        UnregisterError, UpdateError, WitnessSlice,
    },
    store::GovernanceActivity,
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

    #[error("incorrect stake registration deposit: provided {provided}, expected {expected}")]
    IncorrectStakeDeposit { provided: Lovelace, expected: Lovelace },

    #[error("incorrect drep registration deposit: provided {provided}, expected {expected}")]
    IncorrectDRepDeposit { provided: Lovelace, expected: Lovelace },
}

pub(crate) fn execute<C>(
    context: &mut C,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: &GovernanceActivity,
    transaction: TransactionPointer,
    certificates: Option<NonEmptySet<Certificate>>,
) -> Result<(), InvalidCertificates>
where
    C: PoolsSlice + AccountsSlice + DRepsSlice + CommitteeSlice + WitnessSlice,
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

// FIXME: Perform all necessary rules validations down here.
fn execute_one<C>(
    context: &mut C,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: &GovernanceActivity,
    pointer: CertificatePointer,
    certificate: Certificate,
) -> Result<(), InvalidCertificates>
where
    C: PoolsSlice + AccountsSlice + DRepsSlice + CommitteeSlice + WitnessSlice,
{
    // Promote a ScriptHash into a RequiredScript, with additional context needed to defer the
    // validation of the script.
    let into_required_script = |hash: Hash<SCRIPT>| -> RequiredScript {
        RequiredScript {
            hash,
            index: pointer.certificate_index as u32,
            purpose: ScriptPurpose::Cert,
            datum: MemoizedDatum::None,
        }
    };

    match certificate {
        Certificate::PoolRegistration {
            operator: id,
            vrf_keyhash: vrf,
            pledge,
            cost,
            margin,
            reward_account,
            pool_owners: owners,
            relays,
            pool_metadata: metadata,
        } => {
            context.require_vkey_witness(id);
            // https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/shelley/impl/src/Cardano/Ledger/Shelley/UTxO.hs#L250-L256
            // The Haskell node requires both the owners and the operators, which may be the same pkh.
            // TODO: We need coverage for this branch, we have none in either conformance tests or unit tests.
            for owner in owners.iter() {
                context.require_vkey_witness(*owner);
            }

            let reward_account_network =
                parse_reward_account(&reward_account).ok_or(InvalidCertificates::PoolMalformedRewardAccount)?.1;
            if reward_account_network != network {
                return Err(InvalidCertificates::PoolWrongNetwork {
                    expected: network,
                    actual: reward_account_network,
                });
            }

            if cost < protocol_parameters.min_pool_cost {
                return Err(InvalidCertificates::PoolCostTooLow {
                    provided: cost,
                    minimum: protocol_parameters.min_pool_cost,
                });
            }

            let params = PoolParams { id, vrf, pledge, cost, margin, reward_account, owners, relays, metadata };
            PoolsSlice::register(context, params, pointer);
            Ok(())
        }

        Certificate::PoolRetirement(id, epoch) => {
            context.require_vkey_witness(id);

            // NOTE: Some conformance tests fail this check because the Haskell imp tests run on
            // a synthetic test chain whose epoch/slot mapping differs from our era_history. Our
            // slot_to_epoch computes a different current epoch, making the range check reject
            // transactions that the Haskell node accepts.
            let current_epoch = era_history.slot_to_epoch_unchecked_horizon(pointer.slot())?;
            let retirement_epoch = Epoch::from(epoch);
            let max_epoch = current_epoch + protocol_parameters.stake_pool_max_retirement_epoch;
            if retirement_epoch <= current_epoch || retirement_epoch > max_epoch {
                return Err(InvalidCertificates::PoolRetirementWrongEpoch {
                    epoch: retirement_epoch,
                    current_epoch,
                    max_epoch,
                });
            }

            PoolsSlice::retire(context, id, Epoch::from(epoch));
            Ok(())
        }

        Certificate::StakeRegistration(credential) => {
            AccountsSlice::register(
                context,
                credential,
                AccountState { deposit: protocol_parameters.stake_credential_deposit, pool: None, drep: None },
            )?;
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

            AccountsSlice::register(context, credential, AccountState { deposit, pool: None, drep: None })?;
            Ok(())
        }

        Certificate::StakeDeregistration(credential) | Certificate::UnReg(credential, _) => {
            match credential {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };
            AccountsSlice::unregister(context, credential);
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

            let valid_until = if protocol_parameters.protocol_version <= PROTOCOL_VERSION_9 {
                era_history.slot_to_epoch_unchecked_horizon(pointer.slot())? + protocol_parameters.drep_expiry
            } else {
                era_history.slot_to_epoch_unchecked_horizon(pointer.slot())? + protocol_parameters.drep_expiry
                    - governance_activity.consecutive_dormant_epochs as u64
            };

            DRepsSlice::register(
                context,
                drep,
                DRepRegistration { deposit, registered_at: pointer, valid_until },
                Option::from(anchor),
            )?;
            Ok(())
        }

        Certificate::UnRegDRepCert(drep, refund) => {
            match drep {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };
            DRepsSlice::unregister(context, drep, refund, pointer);
            Ok(())
        }

        Certificate::UpdateDRepCert(drep, anchor) => {
            match drep {
                StakeCredential::ScriptHash(hash) => context.require_script_witness(into_required_script(hash)),
                StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
            };
            DRepsSlice::update(context, drep, Option::from(anchor))?;
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
            CommitteeSlice::resign(context, cold_credential, Option::from(anchor))?;
            Ok(())
        }

        Certificate::StakeVoteDeleg(credential, pool, drep) => {
            let drep_deleg = Certificate::VoteDeleg(credential.clone(), drep);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, drep_deleg)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, pool_deleg)
        }

        Certificate::StakeRegDeleg(credential, pool, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, reg)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, pool_deleg)
        }

        Certificate::StakeVoteRegDeleg(credential, pool, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, reg)?;
            let pool_deleg = Certificate::StakeDelegation(credential.clone(), pool);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, pool_deleg)?;
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, drep_deleg)
        }

        Certificate::VoteRegDeleg(credential, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, reg)?;
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute_one(context, network, protocol_parameters, era_history, governance_activity, pointer, drep_deleg)
        }
    }
}
