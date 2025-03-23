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

use crate::context::{
    AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepState, DRepsSlice, DelegateError,
    PoolsSlice, RegisterError, UnregisterError, UpdateError,
};
use amaru_kernel::{
    Certificate, CertificatePointer, DRep, Lovelace, PoolId, PoolParams, StakeCredential,
    STAKE_CREDENTIAL_DEPOSIT,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidCertificate {
    #[error("stake credential already registered: {0}")]
    StakeCredentialAlreadyRegistered(#[from] RegisterError<AccountState, StakeCredential>),

    #[error("invalid stake credential pool delegation: {0}")]
    StakeCredentialInvalidPoolDelegation(#[from] DelegateError<StakeCredential, PoolId>),

    #[error("invalid stake credential vote delegation: {0}")]
    StakeCredentialInvalidVoteDelegation(#[from] DelegateError<StakeCredential, DRep>),

    #[error("drep already registered: {0}")]
    DRepAlreadyRegistered(#[from] RegisterError<DRepState, StakeCredential>),

    #[error("invalid drep attempted update: {0}")]
    DRepInvalidUpdate(#[from] UpdateError<StakeCredential>),

    #[error("unknown cc member: {0}")]
    CCMemberUnknown(#[from] UnregisterError<CCMember, StakeCredential>),

    #[error("invalid cc member hot credential delegation: {0}")]
    CCMemberInvalidDelegation(#[from] DelegateError<StakeCredential, StakeCredential>),
}

// FIXME: Perform all necessary rules validations down here.
pub(crate) fn execute<C>(
    context: &mut C,
    certificate: Certificate,
    pointer: CertificatePointer,
) -> Result<(), InvalidCertificate>
where
    C: PoolsSlice + AccountsSlice + DRepsSlice + CommitteeSlice,
{
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
            let params = PoolParams {
                id,
                vrf,
                pledge,
                cost,
                margin,
                reward_account,
                owners,
                relays,
                metadata,
            };
            PoolsSlice::register(context, params);
            Ok(())
        }

        Certificate::PoolRetirement(id, epoch) => {
            PoolsSlice::retire(context, id, epoch);
            Ok(())
        }

        Certificate::StakeRegistration(credential) => {
            AccountsSlice::register(
                context,
                credential,
                AccountState {
                    deposit: STAKE_CREDENTIAL_DEPOSIT as Lovelace,
                    pool: None,
                    drep: None,
                },
            )?;
            Ok(())
        }

        Certificate::Reg(credential, deposit) => {
            AccountsSlice::register(
                context,
                credential,
                AccountState {
                    deposit,
                    pool: None,
                    drep: None,
                },
            )?;
            Ok(())
        }

        Certificate::StakeDeregistration(credential) | Certificate::UnReg(credential, _) => {
            AccountsSlice::unregister(context, credential);
            Ok(())
        }

        Certificate::StakeDelegation(credential, pool) => {
            context.delegate_pool(credential, pool)?;
            Ok(())
        }

        Certificate::RegDRepCert(drep, deposit, anchor) => {
            DRepsSlice::register(
                context,
                drep,
                DRepState {
                    deposit,
                    registered_at: pointer,
                    anchor: Option::from(anchor),
                },
            )?;
            Ok(())
        }

        Certificate::UnRegDRepCert(drep, refund) => {
            DRepsSlice::unregister(context, drep, refund);
            Ok(())
        }

        Certificate::UpdateDRepCert(drep, anchor) => {
            DRepsSlice::update(context, drep, Option::from(anchor))?;
            Ok(())
        }

        Certificate::VoteDeleg(credential, drep) => {
            AccountsSlice::delegate_vote(context, credential, drep, pointer)?;
            Ok(())
        }

        Certificate::AuthCommitteeHot(cold_credential, hot_credential) => {
            CommitteeSlice::delegate_cold_key(context, cold_credential, hot_credential)?;
            Ok(())
        }

        Certificate::ResignCommitteeCold(cold_credential, anchor) => {
            CommitteeSlice::resign(context, cold_credential, Option::from(anchor))?;
            Ok(())
        }

        Certificate::StakeVoteDeleg(credential, pool, drep) => {
            let drep_deleg = Certificate::VoteDeleg(credential.clone(), drep);
            execute(context, drep_deleg, pointer)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute(context, pool_deleg, pointer)
        }

        Certificate::StakeRegDeleg(credential, pool, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            execute(context, reg, pointer)?;
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            execute(context, pool_deleg, pointer)
        }

        Certificate::StakeVoteRegDeleg(credential, pool, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            execute(context, reg, pointer)?;
            let pool_deleg = Certificate::StakeDelegation(credential.clone(), pool);
            execute(context, pool_deleg, pointer)?;
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute(context, drep_deleg, pointer)
        }

        Certificate::VoteRegDeleg(credential, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            execute(context, reg, pointer)?;
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            execute(context, drep_deleg, pointer)
        }
    }
}
