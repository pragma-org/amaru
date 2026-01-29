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

use crate::cbor;
use std::{cmp::Ordering, fmt, ops::Deref};

pub use pallas_primitives::conway::GovActionId as ProposalId;

// TODO: This type shouldn't exist, and `Ord` / `PartialOrd` should be derived in Pallas on
// 'GovActionId' already.
#[derive(Debug, Eq, PartialEq, Clone)]
#[repr(transparent)]
pub struct ComparableProposalId {
    pub inner: ProposalId,
}

impl ComparableProposalId {
    /// Like `Display`, but more compact
    pub fn to_compact_string(&self) -> String {
        format!(
            "{}.{}",
            self.inner.action_index,
            self.inner
                .transaction_id
                .to_string()
                .chars()
                .take(8)
                .collect::<String>()
        )
    }
}

impl AsRef<ComparableProposalId> for ComparableProposalId {
    fn as_ref(&self) -> &ComparableProposalId {
        self
    }
}

impl Deref for ComparableProposalId {
    type Target = ProposalId;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl fmt::Display for ComparableProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}#{}",
            self.inner.transaction_id, self.inner.action_index,
        )
    }
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

impl<C> cbor::encode::Encode<C> for ComparableProposalId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(&self.inner, ctx)?;
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for ComparableProposalId {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(Self {
            inner: d.decode_with(ctx)?,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::{ComparableProposalId, ProposalId};
    use crate::{Hash, prop_cbor_roundtrip};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(ComparableProposalId, any_comparable_proposal_id());

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

    prop_compose! {
        pub fn any_comparable_proposal_id()(
            inner in any_proposal_id()
        ) -> ComparableProposalId {
            ComparableProposalId {
                inner,
            }
        }
    }
}
