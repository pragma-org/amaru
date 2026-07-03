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

use crate::{Anchor, ConstitutionalCommitteeMemberStatus, StakeCredential, StrictMaybe};

/// A committee member's authorization, mirroring Haskell's `CommitteeAuthorization`. This is the
/// amaru-internal representation of the on-chain `ConstitutionalCommitteeMemberStatus`, using plain
/// `Option` rather than the wire-only `StrictMaybe`.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitteeAuthorization {
    /// The member has authorized this hot credential.
    HotCredential(StakeCredential),
    /// The member has resigned, with an optional rationale anchor.
    Resigned(Option<Anchor>),
}

impl From<&ConstitutionalCommitteeMemberStatus> for CommitteeAuthorization {
    fn from(status: &ConstitutionalCommitteeMemberStatus) -> Self {
        match status {
            ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(hot) => {
                CommitteeAuthorization::HotCredential(hot.clone())
            }
            ConstitutionalCommitteeMemberStatus::Resigned(StrictMaybe::Nothing) => {
                CommitteeAuthorization::Resigned(None)
            }
            ConstitutionalCommitteeMemberStatus::Resigned(StrictMaybe::Just(anchor)) => {
                CommitteeAuthorization::Resigned(Some(anchor.clone()))
            }
        }
    }
}
