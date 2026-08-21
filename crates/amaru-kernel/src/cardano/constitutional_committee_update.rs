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

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use crate::{Credential, Epoch, SafeRatio};

#[derive(Debug, Clone)]
pub enum ConstitutionalCommitteeUpdate {
    NoConfidence,
    ChangeMembers { removed: BTreeSet<Credential>, added: BTreeMap<Credential, Epoch>, threshold: SafeRatio },
}

impl fmt::Display for ConstitutionalCommitteeUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfidence => write!(f, "no-confidence"),
            Self::ChangeMembers { removed, added, threshold } => {
                let mut need_separator = false;

                if !removed.is_empty() {
                    write!(f, "{} removed", removed.len())?;
                    need_separator = true;
                }

                if !added.is_empty() {
                    write!(f, "{}{} added", if need_separator { ", " } else { "" }, added.len())?;
                    need_separator = true;
                }

                write!(
                    f,
                    "{}threshold={}/{}",
                    if need_separator { ", " } else { "" },
                    threshold.numer(),
                    threshold.denom(),
                )
            }
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{collection, prelude::*};

    use crate::{ConstitutionalCommitteeUpdate, Epoch, any_credential, safe_ratio};

    pub fn any_constitutional_committee_update(
        any_epoch: impl Strategy<Value = Epoch>,
    ) -> impl Strategy<Value = ConstitutionalCommitteeUpdate> {
        let any_no_confidence = Just(ConstitutionalCommitteeUpdate::NoConfidence);

        let any_change_members = (
            any::<u8>(),
            collection::btree_set(any_credential(), 0..3),
            collection::btree_map(any_credential(), any_epoch, 0..3),
        )
            .prop_map(|(numerator, removed, added)| ConstitutionalCommitteeUpdate::ChangeMembers {
                removed,
                added,
                threshold: safe_ratio(numerator as u64, 1),
            });

        prop_oneof![1 => any_no_confidence, 2 => any_change_members]
    }
}
