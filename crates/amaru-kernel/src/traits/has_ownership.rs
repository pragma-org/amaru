// Copyright 2026 PRAGMA
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

use crate::{Certificate, Credential, RewardAccount, ShelleyAddress, Voter};

pub trait HasOwnership {
    /// Returns ownership credential of a given entity, if any.
    fn owner(&self) -> Credential;
}

impl HasOwnership for RewardAccount {
    fn owner(&self) -> Credential {
        self.credential()
    }
}

impl HasOwnership for ShelleyAddress {
    fn owner(&self) -> Credential {
        *self.payment()
    }
}

impl HasOwnership for Voter {
    fn owner(&self) -> Credential {
        match self {
            Self::ConstitutionalCommitteeKey(hash) | Self::DRepKey(hash) | Self::StakePoolKey(hash) => {
                Credential::KeyHash(*hash)
            }
            Self::ConstitutionalCommitteeScript(hash) | Self::DRepScript(hash) => Credential::ScriptHash(*hash),
        }
    }
}

impl HasOwnership for Certificate {
    fn owner(&self) -> Credential {
        match self {
            Self::StakeRegistration(stake_credential)
            | Self::StakeDeregistration(stake_credential)
            | Self::StakeDelegation(stake_credential, _)
            | Self::Reg(stake_credential, _)
            | Self::UnReg(stake_credential, _)
            | Self::VoteDeleg(stake_credential, _)
            | Self::StakeVoteDeleg(stake_credential, _, _)
            | Self::StakeRegDeleg(stake_credential, _, _)
            | Self::VoteRegDeleg(stake_credential, _, _)
            | Self::StakeVoteRegDeleg(stake_credential, _, _, _)
            | Self::AuthCommitteeHot(stake_credential, _)
            | Self::ResignCommitteeCold(stake_credential, _)
            | Self::RegDRepCert(stake_credential, _, _)
            | Self::UnRegDRepCert(stake_credential, _)
            | Self::UpdateDRepCert(stake_credential, _) => *stake_credential,
            Self::PoolRetirement(id, _) => Credential::KeyHash(*id),
            Self::PoolRegistration(params) => Credential::KeyHash(params.id),
        }
    }
}
