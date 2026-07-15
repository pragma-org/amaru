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

use crate::{AuxiliaryData, TransactionBody, TransactionId, WitnessSet, cbor};

// TODO:
//
// Think about what public API we wanna expose. Exposing
// all fields an internals doesn't sound like a good idea and will likely break people's code
// (including ours) over time.
#[derive(Debug, Clone, PartialEq, Eq, cbor::Encode, serde::Serialize, serde::Deserialize)]
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

// NOTE: Do not macro-derive this one
//
// minicbor allows Option fields to be omitted; whereas the CDDL explicitly requires a null marker.
impl<'d, C> cbor::decode::Decode<'d, C> for Transaction {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(4)?;
            Ok(Self {
                body: d.decode_with(ctx)?,
                witnesses: d.decode_with(ctx)?,
                is_expected_valid: d.decode_with(ctx)?,
                auxiliary_data: d.decode_with(ctx)?,
            })
        })
    }
}

impl Transaction {
    pub fn tx_id(&self) -> TransactionId {
        TransactionId::new(self.body.id())
    }
}
