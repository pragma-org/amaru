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
    fmt,
    fmt::{Display, Formatter},
    ops::Deref,
};

use crate::{Hash, cbor, size::TRANSACTION_BODY};

/// Identifier for a transaction. This is the hash of the transaction body bytes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    std::hash::Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[schemars(transparent)]
#[repr(transparent)]
pub struct TransactionId(Hash<{ TRANSACTION_BODY }>);

impl TransactionId {
    pub fn new(id: Hash<{ TRANSACTION_BODY }>) -> TransactionId {
        TransactionId(id)
    }

    /// Returns a short hex representation (first 6 bytes / 12 chars) suitable for log lines
    /// and multi-id collection rendering where the full hash would be too noisy.
    pub fn short(&self) -> String {
        hex::encode(&self.0.as_slice()[..6])
    }
}

impl Display for TransactionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}

impl Deref for TransactionId {
    type Target = Hash<{ TRANSACTION_BODY }>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl cbor::Encode<()> for TransactionId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.deref().encode(e, ctx)
    }
}

impl<'b> cbor::Decode<'b, ()> for TransactionId {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        d.decode_with(ctx).map(Self)
    }
}
