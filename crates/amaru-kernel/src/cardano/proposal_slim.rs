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

use std::{fmt, mem, str::FromStr};

use crate::{GovernanceAction, ProtocolVersion};

/// A slim view of a [`GovernanceAction`], holding only what a proposal needs to know about the one
/// it chains onto: the lineage it belongs to and, for hard forks, the protocol version it would
/// enact, which the next hard fork of that lineage must be able to follow.
///
/// Each variant is a lineage; use [`ProposalSlim::same_lineage`] to compare two proposals
/// irrespective of what they carry.
///
/// This stands in for governance actions wherever the volatile window and the validation context
/// would otherwise carry their full payload, so it should stay small and `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "&str", into = "String")]
pub enum ProposalSlim {
    ProtocolParameters,
    HardFork(ProtocolVersion),
    ConstitutionalCommittee,
    Constitution,
    Orphan,
}

impl ProposalSlim {
    /// Whether both proposals chain onto the same lineage, regardless of what they carry.
    pub fn same_lineage(self, other: Self) -> bool {
        mem::discriminant(&self) == mem::discriminant(&other)
    }
}

impl TryFrom<&str> for ProposalSlim {
    type Error = <ProposalSlim as FromStr>::Err;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        ProposalSlim::from_str(s)
    }
}

impl From<ProposalSlim> for String {
    fn from(kind: ProposalSlim) -> Self {
        kind.to_string()
    }
}

impl fmt::Display for ProposalSlim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtocolParameters => write!(f, "ProtocolParameters"),
            Self::HardFork(version) => write!(f, "HardFork({version})"),
            Self::ConstitutionalCommittee => write!(f, "ConstitutionalCommittee"),
            Self::Constitution => write!(f, "Constitution"),
            Self::Orphan => write!(f, "Orphan"),
        }
    }
}

impl FromStr for ProposalSlim {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ProtocolParameters" => Ok(Self::ProtocolParameters),
            "ConstitutionalCommittee" => Ok(Self::ConstitutionalCommittee),
            "Constitution" => Ok(Self::Constitution),
            "Orphan" => Ok(Self::Orphan),
            s => match s.strip_prefix("HardFork(").and_then(|s| s.strip_suffix(")")) {
                Some(version) => ProtocolVersion::from_str(version).map(Self::HardFork),
                None => Err(s.to_string()),
            },
        }
    }
}

impl From<&GovernanceAction> for ProposalSlim {
    fn from(action: &GovernanceAction) -> Self {
        use GovernanceAction::*;
        match action {
            ParameterChange(..) => Self::ProtocolParameters,
            HardForkInitiation(_, version) => Self::HardFork(*version),
            UpdateCommittee(..) | NoConfidence(..) => Self::ConstitutionalCommittee,
            NewConstitution(..) => Self::Constitution,
            TreasuryWithdrawals(..) | Information => Self::Orphan,
        }
    }
}
