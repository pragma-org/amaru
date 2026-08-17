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

use std::fmt;

use crate::{Hash, MaxString128, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
pub struct PoolMetadata {
    // NOTE: keep fields in lexicographic order
    //
    // The serde instance is used for canonical ledger state comparisons.
    #[n(1)]
    pub content_hash: Hash<32>,
    #[n(0)]
    pub url: MaxString128,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for PoolMetadata {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self { url: d.decode_with(ctx)?, content_hash: d.decode_with(ctx)? })
        })
    }
}

impl fmt::Display for PoolMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]: {}", self.content_hash, self.url)
    }
}
