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

use std::{fmt, str::FromStr};

use crate::GovernanceAction;

/// A type capturing just the proposal group/kind another proposal belong. The kind determines the
/// lineage the proposal belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "&str", into = "String")]
pub enum ProposalKind {
    ProtocolParameters,
    HardFork,
    ConstitutionalCommittee,
    Constitution,
    Orphan,
}

impl TryFrom<&str> for ProposalKind {
    type Error = <ProposalKind as FromStr>::Err;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        ProposalKind::from_str(s)
    }
}

impl From<ProposalKind> for String {
    fn from(kind: ProposalKind) -> Self {
        kind.to_string()
    }
}

impl fmt::Display for ProposalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::ProtocolParameters => "ProtocolParameters",
                Self::HardFork => "HardFork",
                Self::ConstitutionalCommittee => "ConstitutionalCommittee",
                Self::Constitution => "Constitution",
                Self::Orphan => "Orphan",
            }
        )
    }
}

impl FromStr for ProposalKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ProtocolParameters" => Ok(Self::ProtocolParameters),
            "HardFork" => Ok(Self::HardFork),
            "ConstitutionalCommittee" => Ok(Self::ConstitutionalCommittee),
            "Constitution" => Ok(Self::Constitution),
            "Orphan" => Ok(Self::Orphan),
            s => Err(s.to_string()),
        }
    }
}

impl From<&GovernanceAction> for ProposalKind {
    fn from(action: &GovernanceAction) -> Self {
        use GovernanceAction::*;
        match action {
            ParameterChange(..) => Self::ProtocolParameters,
            HardForkInitiation(..) => Self::HardFork,
            UpdateCommittee(..) | NoConfidence(..) => Self::ConstitutionalCommittee,
            NewConstitution(..) => Self::Constitution,
            TreasuryWithdrawals(..) | Information => Self::Orphan,
        }
    }
}
