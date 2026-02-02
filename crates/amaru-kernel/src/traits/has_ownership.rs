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

use crate::{
    Certificate, ShelleyAddress, ShelleyPaymentPart, StakeAddress, StakeCredential, StakePayload,
    Voter,
};

pub trait HasOwnership {
    /// Returns ownership credential of a given entity, if any.
    ///
    /// TODO: The return type is slightly misleading; we refer to a 'StakeCredential', whereas the
    /// underlying method mainly targets payment credentials in addresses. The reason for this side
    /// step is that there's no 'Credential' type in Pallas unforunately, and so we just borrow the
    /// structure of 'StakeCredential'.
    fn owner(&self) -> StakeCredential;
}

impl HasOwnership for StakeAddress {
    fn owner(&self) -> StakeCredential {
        match self.payload() {
            StakePayload::Stake(hash) => StakeCredential::AddrKeyhash(*hash),
            StakePayload::Script(hash) => StakeCredential::ScriptHash(*hash),
        }
    }
}

impl HasOwnership for ShelleyAddress {
    fn owner(&self) -> StakeCredential {
        match self.payment() {
            ShelleyPaymentPart::Key(hash) => StakeCredential::AddrKeyhash(*hash),
            ShelleyPaymentPart::Script(hash) => StakeCredential::ScriptHash(*hash),
        }
    }
}

impl HasOwnership for Voter {
    fn owner(&self) -> StakeCredential {
        match self {
            Self::ConstitutionalCommitteeKey(hash)
            | Self::DRepKey(hash)
            | Self::StakePoolKey(hash) => StakeCredential::AddrKeyhash(*hash),
            Self::ConstitutionalCommitteeScript(hash) | Self::DRepScript(hash) => {
                StakeCredential::ScriptHash(*hash)
            }
        }
    }
}

impl HasOwnership for Certificate {
    fn owner(&self) -> StakeCredential {
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
            | Self::UpdateDRepCert(stake_credential, _) => stake_credential.clone(),
            Self::PoolRegistration { operator: id, .. } | Self::PoolRetirement(id, _) => {
                StakeCredential::AddrKeyhash(*id)
            }
        }
    }
}
