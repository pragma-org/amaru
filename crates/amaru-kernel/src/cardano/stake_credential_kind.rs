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

use crate::{StakeCredential, Voter};

#[derive(Debug)]
pub enum StakeCredentialKind {
    VerificationKey,
    Script,
}

impl std::fmt::Display for StakeCredentialKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::VerificationKey => "verification_key",
            Self::Script => "script",
        })
    }
}

impl From<&StakeCredential> for StakeCredentialKind {
    fn from(credential: &StakeCredential) -> Self {
        match credential {
            StakeCredential::AddrKeyhash(..) => Self::VerificationKey,
            StakeCredential::ScriptHash(..) => Self::Script,
        }
    }
}

impl From<&Voter> for StakeCredentialKind {
    fn from(voter: &Voter) -> Self {
        match voter {
            Voter::DRepKey(..)
            | Voter::ConstitutionalCommitteeKey(..)
            | Voter::StakePoolKey(..) => Self::VerificationKey,
            Voter::DRepScript(..) | Voter::ConstitutionalCommitteeScript(..) => Self::Script,
        }
    }
}
