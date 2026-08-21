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

use crate::{AuxiliaryData, TransactionBody, TransactionId, WitnessSet, cbor::WithSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TransactionRef<'a> {
    pub body: &'a TransactionBody,
    pub witnesses: WithSize<&'a WitnessSet>,
    pub is_expected_valid: bool,
    pub auxiliary_data: Option<&'a AuxiliaryData>,
}

impl<'a> TransactionRef<'a> {
    pub fn tx_id(&self) -> TransactionId {
        TransactionId::new(self.body.id())
    }

    // NOTE: Transaction size calculation
    //
    // Due to how the transactions are serialised in blocks (with seggregated witnesses
    // and auxiliary data), we have to calculate the size from multiple pieces and add
    // an extra 'cbor framing byte' which corresponds to the declaration of the
    // top-level array of size 3 (`0x83`). Importantly, the validity of the transaction
    // is not taken into account for the size calculation (rationale being that this
    // the logic is then preserved between pre-alonzo and post-alonzo eras).
    //
    // See also: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Tx.hs#L351-L362>
    #[expect(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        let aux_data_len = self.auxiliary_data.as_ref().map(|data| data.len()).unwrap_or(1);
        1 + self.body.len() + self.witnesses.len() as u64 + aux_data_len
    }
}
