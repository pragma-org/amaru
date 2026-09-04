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

use crate::{AuxiliaryData, TransactionBody, TransactionId, TransactionRef, WitnessSet, cbor, cbor::WithSize};

// TODO:
//
// Think about what public API we wanna expose. Exposing
// all fields an internals doesn't sound like a good idea and will likely break people's code
// (including ours) over time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Transaction {
    pub body: TransactionBody,
    pub witnesses: WithSize<WitnessSet>,
    pub is_expected_valid: bool,
    pub auxiliary_data: Option<AuxiliaryData>,
}

impl Transaction {
    #[expect(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        TransactionRef {
            body: &self.body,
            auxiliary_data: self.auxiliary_data.as_ref(),
            witnesses: self.witnesses.as_ref(),
            is_expected_valid: self.is_expected_valid,
        }
        .len()
    }
}

// NOTE: Do not macro-derive the CBOR instances.
//
// minicbor omits a trailing 'None' Option when serialising an array, whereas the CDDL
// explicitly requires the auxiliary_data slot to always be present as a null marker.
impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for Transaction {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?;
        e.encode_with(&self.body, ctx)?;
        e.encode_with(&self.witnesses, ctx)?;
        e.encode_with(self.is_expected_valid, ctx)?;
        match &self.auxiliary_data {
            Some(auxiliary_data) => e.encode_with(auxiliary_data, ctx)?,
            None => e.null()?,
        };
        Ok(())
    }
}

impl<'d, C: cbor::HasProtocolVersion> cbor::decode::Decode<'d, C> for Transaction {
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

    pub fn tx_ref(&self) -> TransactionRef<'_> {
        TransactionRef {
            body: &self.body,
            witnesses: self.witnesses.as_ref(),
            is_expected_valid: self.is_expected_valid,
            auxiliary_data: self.auxiliary_data.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Transaction, include_cbor, to_cbor};

    #[test]
    fn size_without_reencoding() {
        // A transaction constructed using Eternl, whose choice of encoding differs in a few aspects
        // (indefinite vs definite, ...). So it's a good candidate for size/re-encoding validations.
        let transaction: Transaction =
            include_cbor!("transaction.len/99949da314af224cff611d22feece9e6f150ad232f6b421e9294c74aae0d5d81.cbor");
        assert_eq!(transaction.len(), 293);
        assert_eq!(to_cbor(&transaction).len(), 297);
    }
}
