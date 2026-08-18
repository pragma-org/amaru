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

use crate::{GovernanceAction, ProtocolParamUpdate, ProtocolVersion};

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
    ProtocolParameters(AnyInSecurityGroup),
    HardFork(ProtocolVersion),
    Constitution,
    ConstitutionalCommittee,
    Orphan(IsTreasuryWithdrawals),
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
            Self::ProtocolParameters(any_in_security_group) if bool::from(*any_in_security_group) => {
                write!(f, "ProtocolParameters(security-group)")
            }
            Self::ProtocolParameters(..) => {
                write!(f, "ProtocolParameters")
            }
            Self::HardFork(version) => write!(f, "HardFork({version})"),
            Self::ConstitutionalCommittee => write!(f, "ConstitutionalCommittee"),
            Self::Constitution => write!(f, "Constitution"),
            Self::Orphan(is_treasury_withdrawals) if bool::from(*is_treasury_withdrawals) => {
                write!(f, "TreasuryWithdrawals")
            }
            Self::Orphan(..) => write!(f, "Information"),
        }
    }
}

impl FromStr for ProposalSlim {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ProtocolParameters" => Ok(Self::ProtocolParameters(false.into())),
            "ProtocolParameters(security-group)" => Ok(Self::ProtocolParameters(true.into())),
            "ConstitutionalCommittee" => Ok(Self::ConstitutionalCommittee),
            "Constitution" => Ok(Self::Constitution),
            "Orphan" | "Information" => Ok(Self::Orphan(false.into())),
            "TreasuryWithdrawals" => Ok(Self::Orphan(true.into())),
            s => {
                if let Some(version) = s.strip_prefix("HardFork(").and_then(|s| s.strip_suffix(")")) {
                    return ProtocolVersion::from_str(version).map(Self::HardFork);
                }

                Err(s.to_string())
            }
        }
    }
}

impl From<&GovernanceAction> for ProposalSlim {
    fn from(action: &GovernanceAction) -> Self {
        use GovernanceAction::*;
        match action {
            ParameterChange(_, update, _) => Self::ProtocolParameters(update.as_ref().into()),
            HardForkInitiation(_, version) => Self::HardFork(*version),
            TreasuryWithdrawals(..) => Self::Orphan(true.into()),
            Information => Self::Orphan(false.into()),
            NoConfidence(..) | UpdateCommittee(..) => Self::ConstitutionalCommittee,
            NewConstitution(..) => Self::Constitution,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct IsTreasuryWithdrawals(bool);

impl fmt::Display for IsTreasuryWithdrawals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<bool> for IsTreasuryWithdrawals {
    fn from(true_or_false: bool) -> Self {
        Self(true_or_false)
    }
}

impl From<IsTreasuryWithdrawals> for bool {
    fn from(IsTreasuryWithdrawals(true_or_false): IsTreasuryWithdrawals) -> Self {
        true_or_false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct AnyInSecurityGroup(bool);

impl fmt::Display for AnyInSecurityGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&ProtocolParamUpdate> for AnyInSecurityGroup {
    fn from(update: &ProtocolParamUpdate) -> AnyInSecurityGroup {
        Self(update.any_in_security_group())
    }
}

impl From<bool> for AnyInSecurityGroup {
    fn from(true_or_false: bool) -> Self {
        Self(true_or_false)
    }
}

impl From<AnyInSecurityGroup> for bool {
    fn from(AnyInSecurityGroup(true_or_false): AnyInSecurityGroup) -> Self {
        true_or_false
    }
}
