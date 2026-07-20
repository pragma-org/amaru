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

use pallas_crypto::key::ed25519;

use crate::{AuxiliaryData, Bytes, TransactionBody, TransactionId, WitnessSet, cbor};

const CHAIN_CODE_SIZE: usize = 32;

// TODO:
//
// Think about what public API we wanna expose. Exposing
// all fields an internals doesn't sound like a good idea and will likely break people's code
// (including ours) over time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Transaction {
    pub body: TransactionBody,
    pub witnesses: WitnessSet,
    pub is_expected_valid: bool,
    pub auxiliary_data: Option<AuxiliaryData>,
}

// NOTE: Do not macro-derive the CBOR instances.
//
// minicbor omits a trailing 'None' Option when serialising an array, whereas the CDDL
// explicitly requires the auxiliary_data slot to always be present as a null marker.
impl<C> cbor::Encode<C> for Transaction {
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

impl<'d, C> cbor::decode::Decode<'d, C> for Transaction {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(4)?;
            let transaction = Self {
                body: d.decode_with(ctx)?,
                witnesses: d.decode_with(ctx)?,
                is_expected_valid: d.decode_with(ctx)?,
                auxiliary_data: d.decode_with(ctx)?,
            };
            assert_sized_witnesses(&transaction.witnesses)?;
            Ok(transaction)
        })
    }
}

/// Ed25519 keys and signatures have a fixed size which we check here.
///
/// TODO: We should move this validation to the Witness decoding once we move from `pallas` our
/// our own data types.
fn assert_sized_witnesses(witnesses: &WitnessSet) -> Result<(), cbor::decode::Error> {
    if let Some(vkey_witnesses) = witnesses.vkeywitness.as_deref() {
        for witness in vkey_witnesses {
            assert_bytes_len("verification key", &witness.vkey, ed25519::PublicKey::SIZE)?;
            assert_bytes_len("verification key signature", &witness.signature, ed25519::Signature::SIZE)?;
        }
    }

    if let Some(bootstrap_witnesses) = witnesses.bootstrap_witness.as_deref() {
        for witness in bootstrap_witnesses {
            assert_bytes_len("bootstrap public key", &witness.public_key, ed25519::PublicKey::SIZE)?;
            assert_bytes_len("bootstrap signature", &witness.signature, ed25519::Signature::SIZE)?;
            assert_bytes_len("bootstrap chain code", &witness.chain_code, CHAIN_CODE_SIZE)?;
        }
    }

    Ok(())
}

fn assert_bytes_len(field: &str, bytes: &Bytes, expected: usize) -> Result<(), cbor::decode::Error> {
    let actual = bytes.len();
    if actual == expected {
        Ok(())
    } else {
        Err(cbor::decode::Error::message(format!("invalid {field} length: expected {expected} bytes but got {actual}")))
    }
}

impl Transaction {
    pub fn tx_id(&self) -> TransactionId {
        TransactionId::new(self.body.id())
    }
}
