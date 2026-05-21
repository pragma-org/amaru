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
};

use pallas_crypto::hash::Hash;

use crate::{cbor, size::TRANSACTION_BODY};

/// Identifier for a transaction. This is the hash of the transaction body bytes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct TransactionId(Hash<{ TRANSACTION_BODY }>);

impl TransactionId {
    pub fn new(id: Hash<{ TRANSACTION_BODY }>) -> TransactionId {
        TransactionId(id)
    }
}

impl AsRef<Hash<{ TRANSACTION_BODY }>> for TransactionId {
    fn as_ref(&self) -> &Hash<{ TRANSACTION_BODY }> {
        &self.0
    }
}

impl cbor::Encode<()> for TransactionId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode(self.0)?;
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for TransactionId {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let hash = Hash::<{ TRANSACTION_BODY }>::decode(d, _ctx)?;
        Ok(TransactionId(hash))
    }
}

impl Display for TransactionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // 6 first bytes = 12 characters
        write!(f, "{}", hex::encode(&self.0.as_slice()[..6]))
    }
}
