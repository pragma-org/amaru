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

use std::{collections::BTreeSet, ops::Deref};

use thiserror::Error;

use crate::{
    Ed25519Signature, IntoNonEmptyUniqueVecError, NonEmptyVec, VerificationKey, cbor, ed25519,
    protocol_version::PROTOCOL_VERSION_12,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct VerificationKeyWitness {
    #[n(0)]
    pub verification_key: VerificationKey,

    #[n(1)]
    pub signature: Ed25519Signature,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for VerificationKeyWitness {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(Self { verification_key: d.decode_with(ctx)?, signature: d.decode_with(ctx)? })
        })
    }
}

/// The ledger sees distinct (key, signature) pairs. Before v12 the wire encoding
/// can contain repetitions, even though the decoded set contains each witness once.
/// Equality retains the original list order and repetitions, as does legacy serialization.
/// See [addrWitsSetDecoder](https://github.com/IntersectMBO/cardano-ledger/blob/6176e413b2be9880c23cedbdaa899f9752693b22/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L616-L626).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "Vec<VerificationKeyWitness>")]
pub struct VerificationKeyWitnesses {
    wire: NonEmptyVec<VerificationKeyWitness>,
    // Allocated only for a legacy witness collection containing repeats.
    distinct: Option<Box<NonEmptyVec<VerificationKeyWitness>>>,
}

impl VerificationKeyWitnesses {
    pub fn singleton(witness: VerificationKeyWitness) -> Self {
        Self { wire: NonEmptyVec::singleton(witness), distinct: None }
    }

    pub fn first(&self) -> &VerificationKeyWitness {
        self.wire.first()
    }

    /// Submitted order, including repeated pairs in a legacy transaction.
    pub fn iter_wire(&self) -> impl Iterator<Item = &VerificationKeyWitness> {
        self.wire.iter()
    }

    /// Yield each distinct pair with its first position in the submitted list.
    pub fn iter_with_positions(&self) -> impl Iterator<Item = (usize, &VerificationKeyWitness)> {
        // The distinct list is in first-occurrence order, so both cursors only advance.
        let mut distinct = self.distinct.as_ref().map(|entries| entries.iter().peekable());
        self.iter_wire()
            .enumerate()
            .filter(move |(_, witness)| distinct.as_mut().is_none_or(|entries| entries.next_if_eq(witness).is_some()))
    }

    fn distinct(&self) -> &NonEmptyVec<VerificationKeyWitness> {
        self.distinct.as_deref().unwrap_or(&self.wire)
    }

    fn from_wire(
        wire: NonEmptyVec<VerificationKeyWitness>,
        reject_duplicates: bool,
    ) -> Result<Self, IntoNonEmptyUniqueVecError> {
        if wire.len() == 1 {
            return Ok(Self { wire, distinct: None });
        }
        let first = wire.first();
        let mut seen = BTreeSet::from([(&first.verification_key, &first.signature)]);
        let mut distinct_tail = None;
        for (i, witness) in wire.iter().enumerate().skip(1) {
            if !seen.insert((&witness.verification_key, &witness.signature)) {
                if reject_duplicates {
                    return Err(IntoNonEmptyUniqueVecError::HasDuplicate);
                }
                distinct_tail.get_or_insert_with(|| wire[1..i].to_vec());
            } else if let Some(tail) = &mut distinct_tail {
                tail.push(witness.clone());
            }
        }
        let distinct = distinct_tail.map(|tail| Box::new(NonEmptyVec::new(first.clone(), tail)));
        Ok(Self { wire, distinct })
    }
}

impl Deref for VerificationKeyWitnesses {
    type Target = [VerificationKeyWitness];

    fn deref(&self) -> &Self::Target {
        self.distinct()
    }
}

impl TryFrom<Vec<VerificationKeyWitness>> for VerificationKeyWitnesses {
    type Error = IntoNonEmptyUniqueVecError;

    fn try_from(values: Vec<VerificationKeyWitness>) -> Result<Self, Self::Error> {
        let wire = NonEmptyVec::try_from(values).map_err(|_| IntoNonEmptyUniqueVecError::Empty)?;
        Self::from_wire(wire, false)
    }
}

impl serde::Serialize for VerificationKeyWitnesses {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire.serialize(serializer)
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for VerificationKeyWitnesses {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let position = d.position();
        let wire = d.decode_with(ctx)?;
        Self::from_wire(wire, ctx.protocol_version() >= PROTOCOL_VERSION_12)
            .map_err(|err| cbor::decode::Error::message(err).at(position))
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for VerificationKeyWitnesses {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let witnesses = if ctx.protocol_version() >= PROTOCOL_VERSION_12 { self.distinct() } else { &self.wire };
        witnesses.encode(e, ctx)
    }
}

#[derive(Debug, Error)]
#[error("invalid signature for given key")]
pub struct InvalidEd25519Signature;

pub fn verify_ed25519_signature(
    verification_key: &VerificationKey,
    signature: &Ed25519Signature,
    message: &[u8],
) -> Result<(), InvalidEd25519Signature> {
    let public_key =
        ed25519::VerifyingKey::try_from(verification_key.as_slice()).map_err(|_| InvalidEd25519Signature)?;

    let signature = ed25519::Signature::from_bytes(signature.as_array());

    public_key.verify_strict(message, &signature).map_err(|_| InvalidEd25519Signature)
}
