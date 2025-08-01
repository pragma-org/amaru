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

pub use pallas_primitives::conway::GovActionId as ProposalId;
use std::cmp::Ordering;

#[derive(Debug, Eq, PartialEq, Clone)]
// TODO: This type shouldn't exist, and `Ord` / `PartialOrd` should be derived in Pallas on
// 'GovActionId' already.
pub struct ComparableProposalId {
    pub inner: ProposalId,
}

impl From<ProposalId> for ComparableProposalId {
    fn from(inner: ProposalId) -> Self {
        Self { inner }
    }
}

impl From<ComparableProposalId> for ProposalId {
    fn from(comparable: ComparableProposalId) -> ProposalId {
        comparable.inner
    }
}

impl PartialOrd for ComparableProposalId {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}

impl Ord for ComparableProposalId {
    fn cmp(&self, rhs: &Self) -> Ordering {
        match self.inner.transaction_id.cmp(&rhs.inner.transaction_id) {
            Ordering::Equal => self.inner.action_index.cmp(&rhs.inner.action_index),
            ordering @ Ordering::Less | ordering @ Ordering::Greater => ordering,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::ProposalId;
    use crate::Hash;
    use proptest::{prelude::*, prop_compose};

    prop_compose! {
        pub fn any_proposal_id()(
            transaction_id in any::<[u8; 32]>(),
            action_index in any::<u32>(),
        ) -> ProposalId {
            ProposalId {
                transaction_id: Hash::new(transaction_id),
                action_index,
            }
        }
    }
}
