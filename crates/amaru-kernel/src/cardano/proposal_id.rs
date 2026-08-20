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

use std::fmt;

use crate::{Hash, cbor, size::TRANSACTION_BODY};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub struct ProposalId {
    pub transaction_id: Hash<{ TRANSACTION_BODY }>,
    pub proposal_index: u32,
}

impl fmt::Display for ProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.transaction_id, self.proposal_index)
    }
}

impl ProposalId {
    /// Like `Display`, but more compact
    pub fn to_compact_string(&self) -> String {
        format!("{}#{}", self.proposal_index, self.transaction_id.to_string().chars().take(8).collect::<String>())
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for ProposalId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.transaction_id, ctx)?;
        e.encode_with(self.proposal_index, ctx)?;
        Ok(())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for ProposalId {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self { transaction_id: d.decode_with(ctx)?, proposal_index: d.decode_with(ctx)? })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{prelude::*, prop_compose};

    use super::ProposalId;
    use crate::{Hash, prop_cbor_roundtrip};

    prop_cbor_roundtrip!(ProposalId, any_proposal_id());

    prop_compose! {
        pub fn any_proposal_id()(
            transaction_id in any::<[u8; 32]>(),
            proposal_index in any::<u32>(),
        ) -> ProposalId {
            ProposalId {
                transaction_id: Hash::new(transaction_id),
                proposal_index,
            }
        }
    }
}
