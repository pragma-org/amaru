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

use std::{collections::BTreeMap, fmt};

use crate::{Credential, Lovelace};

#[derive(Debug, Clone)]
pub enum OrphanProposal {
    TreasuryWithdrawal(BTreeMap<Credential, Lovelace>),
    NicePoll,
}

impl fmt::Display for OrphanProposal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NicePoll => write!(f, "nice poll"),
            Self::TreasuryWithdrawal(withdrawals) => {
                let total =
                    withdrawals.iter().fold(0_u64, |total, (_, single)| total.saturating_add(*single)) / 1_000_000;
                write!(f, "withdrawal={total}₳")
            }
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{collection, prelude::*};

    use crate::{OrphanProposal, any_credential};

    pub fn any_orphan_proposal() -> impl Strategy<Value = OrphanProposal> {
        let any_nice_poll = Just(OrphanProposal::NicePoll);

        let any_treasury_withdrawal = collection::btree_map(any_credential(), 1..(u64::MAX / 3), 1..3)
            .prop_map(OrphanProposal::TreasuryWithdrawal);

        prop_oneof![any_nice_poll, any_treasury_withdrawal]
    }
}
