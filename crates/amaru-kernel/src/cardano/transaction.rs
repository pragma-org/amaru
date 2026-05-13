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

use crate::{AuxiliaryData, TransactionBody, TransactionId, WitnessSet, cardano::hash::size::TRANSACTION_BODY, cbor};

// TODO:
//
// Think about what public API we wanna expose. Exposing
// all fields an internals doesn't sound like a good idea and will likely break people's code
// (including ours) over time.
#[derive(Debug, Clone, PartialEq, Eq, cbor::Encode, cbor::Decode, serde::Serialize, serde::Deserialize)]
pub struct Transaction {
    #[n(0)]
    pub body: TransactionBody,
    #[n(1)]
    pub witnesses: WitnessSet,
    #[n(2)]
    pub is_expected_valid: bool,
    #[n(3)]
    pub auxiliary_data: Option<AuxiliaryData>,
}

impl Transaction {
    pub fn tx_id(&self) -> TxId {
        TxId(self.body.id())
    }
}

/// Identifier for a transaction. This is the hash of the transaction body bytes.
/// It encapsulates the TransactionId to be completely opaque to the structure of the
/// transaction identifier.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TxId(TransactionId);

impl TxId {
    pub fn new(id: TransactionId) -> TxId {
        TxId(id)
    }
}

impl cbor::Encode<()> for TxId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode(self.0)?;
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for TxId {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let hash = Hash::<{ TRANSACTION_BODY }>::decode(d, _ctx)?;
        Ok(TxId(hash))
    }
}

impl Display for TxId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0.as_slice()))
    }
}

pub trait HasTxId {
    fn tx_id(&self) -> TxId;
}

impl HasTxId for Transaction {
    fn tx_id(&self) -> TxId {
        self.tx_id()
    }
}
