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

use serde::ser::SerializeStruct;

use crate::{
    Hash, StakeCredential, cbor,
    size::{KEY, SCRIPT},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum DRep {
    Key(Hash<{ KEY }>),
    Script(Hash<{ SCRIPT }>),
    Abstain,
    NoConfidence,
}

impl<'b, C> cbor::decode::Decode<'b, C> for DRep {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            let variant = d.u16()?;

            match variant {
                0 => {
                    assert_len(2)?;
                    Ok(DRep::Key(d.decode_with(ctx)?))
                }
                1 => {
                    assert_len(2)?;
                    Ok(DRep::Script(d.decode_with(ctx)?))
                }
                2 => {
                    assert_len(1)?;
                    Ok(DRep::Abstain)
                }
                3 => {
                    assert_len(1)?;
                    Ok(DRep::NoConfidence)
                }
                _ => Err(cbor::decode::Error::message("invalid variant id for DRep")),
            }
        })
    }
}

impl<C> cbor::encode::Encode<C> for DRep {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            DRep::Key(h) => {
                e.array(2)?;
                e.encode_with(0, ctx)?;
                e.encode_with(h, ctx)?;

                Ok(())
            }
            DRep::Script(h) => {
                e.array(2)?;
                e.encode_with(1, ctx)?;
                e.encode_with(h, ctx)?;

                Ok(())
            }
            DRep::Abstain => {
                e.array(1)?;
                e.encode_with(2, ctx)?;

                Ok(())
            }
            DRep::NoConfidence => {
                e.array(1)?;
                e.encode_with(3, ctx)?;

                Ok(())
            }
        }
    }
}

#[derive(serde::Serialize)]
#[serde(transparent)]
pub struct AsJson<'a>(#[serde(serialize_with = "serialize")] pub &'a DRep);

pub fn serialize<S: serde::Serializer>(drep: &DRep, serializer: S) -> Result<S::Ok, S::Error> {
    match drep {
        DRep::Abstain => {
            let mut s = serializer.serialize_struct("DRep", 1)?;
            s.serialize_field("type", "abstain")?;
            s
        }
        DRep::NoConfidence => {
            let mut s = serializer.serialize_struct("DRep", 1)?;
            s.serialize_field("type", "no_confidence")?;
            s
        }
        DRep::Script(hash) => {
            let mut s = serializer.serialize_struct("DRep", 2)?;
            // NOTE: keep fields in lexicographic order
            //
            // This instance is used for canonical ledger state comparisons.
            s.serialize_field("hash", &hex::encode(hash))?;
            s.serialize_field("type", "script")?;
            s
        }
        DRep::Key(hash) => {
            let mut s = serializer.serialize_struct("DRep", 2)?;
            // NOTE: keep fields in lexicographic order
            //
            // This instance is used for canonical ledger state comparisons.
            s.serialize_field("hash", &hex::encode(hash))?;
            s.serialize_field("type", "verification_key")?;
            s
        }
    }
    .end()
}

pub fn to_stake_credential(drep: &DRep) -> Option<StakeCredential> {
    match drep {
        DRep::Key(hash) => Some(StakeCredential::AddrKeyhash(*hash)),
        DRep::Script(hash) => Some(StakeCredential::ScriptHash(*hash)),
        DRep::Abstain | DRep::NoConfidence => None,
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{DRep, any_hash28};

    pub fn any_drep() -> impl Strategy<Value = DRep> {
        prop_oneof![
            any_hash28().prop_map(DRep::Key),
            any_hash28().prop_map(DRep::Script),
            Just(DRep::Abstain),
            Just(DRep::NoConfidence),
        ]
    }
}
