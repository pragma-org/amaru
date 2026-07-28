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

use crate::{Bytes, HasScriptHash, Hash, Hasher, MemoizedNativeScript, PlutusScript, cbor, size::SCRIPT};

// ------------------------------------------------------------------------ MemoizedScript

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoizedScript {
    NativeScript(MemoizedNativeScript),
    PlutusV1Script(PlutusScript<1>),
    PlutusV2Script(PlutusScript<2>),
    PlutusV3Script(PlutusScript<3>),
}

impl MemoizedScript {
    #[expect(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        match self {
            Self::NativeScript(script) => script.original_bytes(),
            Self::PlutusV1Script(script) => script.as_ref(),
            Self::PlutusV2Script(script) => script.as_ref(),
            Self::PlutusV3Script(script) => script.as_ref(),
        }
        .len() as u64
    }
}

pub fn serialize_memoized_script<S: serde::ser::Serializer>(
    script: &MemoizedScript,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut s = serializer.serialize_struct("MemoizedScript", 1)?;
    match script {
        // TODO: Adopt a less Rust-tainted encoding one day. Not doing it now because will remand
        // re-generating and re-encoding all the ledger test vectors which is only tangential to
        // the problem I am trying to solve.
        MemoizedScript::NativeScript(native) => {
            s.serialize_field("NativeScript", &hex::encode(native.original_bytes()))?;
        }
        MemoizedScript::PlutusV1Script(plutus) => {
            s.serialize_field("PlutusV1Script", &hex::encode(plutus.as_ref()))?;
        }
        MemoizedScript::PlutusV2Script(plutus) => {
            s.serialize_field("PlutusV2Script", &hex::encode(plutus.as_ref()))?;
        }
        MemoizedScript::PlutusV3Script(plutus) => {
            s.serialize_field("PlutusV3Script", &hex::encode(plutus.as_ref()))?;
        }
    }
    s.end()
}

impl<'b, C> minicbor::Decode<'b, C> for MemoizedScript {
    fn decode(d: &mut minicbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.array()?;

        match d.u8()? {
            0 => Ok(Self::NativeScript(d.decode()?)),
            1 => Ok(Self::PlutusV1Script(d.decode()?)),
            2 => Ok(Self::PlutusV2Script(d.decode()?)),
            3 => Ok(Self::PlutusV3Script(d.decode()?)),
            _ => Err(minicbor::decode::Error::message("invalid variant for MemoizedScript enum")),
        }
    }
}

impl<C> minicbor::Encode<C> for MemoizedScript {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.array(2)?;

        match self {
            MemoizedScript::NativeScript(native) => {
                e.u8(0)?;
                e.encode_with(native, ctx)?;
            }
            MemoizedScript::PlutusV1Script(plutus) => {
                e.u8(1)?;
                e.encode_with(plutus, ctx)?;
            }
            MemoizedScript::PlutusV2Script(plutus) => {
                e.u8(2)?;
                e.encode_with(plutus, ctx)?;
            }
            MemoizedScript::PlutusV3Script(plutus) => {
                e.u8(3)?;
                e.encode_with(plutus, ctx)?;
            }
        };

        Ok(())
    }
}

// ------------------------------------------------------------------------ BorrowedScript

/// A borrowed reference to a script.
///
/// The by-reference counterpart of the owned [`MemoizedScript`], flattened to its four
/// kinds: a native script, or a Plutus script whose language version is carried in the
/// type. The version travels with the script because execution depends on it.
/// The available builtins, the cost model, and the  script-context encoding all differ by Plutus version.
#[derive(Debug, Clone)]
pub enum BorrowedScript<'a> {
    Native(&'a MemoizedNativeScript),
    PlutusV1(&'a PlutusScript<1>),
    PlutusV2(&'a PlutusScript<2>),
    PlutusV3(&'a PlutusScript<3>),
}

impl BorrowedScript<'_> {
    /// Unwraps a layer of CBOR, returning the flat-encoded bytes
    /// that are passed to the CEK machine for evaluation.
    ///
    /// A `BorrowedScript::Native` is treated `unreachable` since there are no redeemers for NativeScript
    /// and they are not flat-encoded bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, cbor::decode::Error> {
        fn decode_cbor_bytes(cbor: &[u8]) -> Result<Vec<u8>, cbor::decode::Error> {
            cbor::decode::Decoder::new(cbor).bytes().map(|b| b.to_vec())
        }

        match self {
            BorrowedScript::PlutusV1(s) => decode_cbor_bytes(s.0.as_ref()),
            BorrowedScript::PlutusV2(s) => decode_cbor_bytes(s.0.as_ref()),
            BorrowedScript::PlutusV3(s) => decode_cbor_bytes(s.0.as_ref()),
            BorrowedScript::Native(_) => unreachable!("a redeemer should never point to a native_script"),
        }
    }
}

impl<'a> From<&'a MemoizedScript> for BorrowedScript<'a> {
    fn from(value: &'a MemoizedScript) -> Self {
        match value {
            MemoizedScript::NativeScript(script) => BorrowedScript::Native(script),
            MemoizedScript::PlutusV1Script(script) => BorrowedScript::PlutusV1(script),
            MemoizedScript::PlutusV2Script(script) => BorrowedScript::PlutusV2(script),
            MemoizedScript::PlutusV3Script(script) => BorrowedScript::PlutusV3(script),
        }
    }
}

impl HasScriptHash for BorrowedScript<'_> {
    fn script_hash(&self) -> Hash<SCRIPT> {
        let (bytes, tag) = match self {
            Self::Native(native) => (native.original_bytes(), 0),
            Self::PlutusV1(plutus) => (plutus.as_ref(), 1),
            Self::PlutusV2(plutus) => (plutus.as_ref(), 2),
            Self::PlutusV3(plutus) => (plutus.as_ref(), 3),
        };

        Hasher::<{ 8 * SCRIPT }>::hash_tagged(bytes, tag)
    }
}

// --------------------------------------------------------------------- PlaceholderScript

#[derive(serde::Deserialize)]
pub(crate) enum PlaceholderScript {
    NativeScript(Bytes),
    PlutusV1(Bytes),
    PlutusV2(Bytes),
    PlutusV3(Bytes),
}

impl TryFrom<PlaceholderScript> for MemoizedScript {
    type Error = String;

    fn try_from(placeholder: PlaceholderScript) -> Result<Self, Self::Error> {
        Ok(match placeholder {
            PlaceholderScript::NativeScript(bytes) => {
                MemoizedScript::NativeScript(MemoizedNativeScript::try_from(bytes)?)
            }
            PlaceholderScript::PlutusV1(bytes) => MemoizedScript::PlutusV1Script(PlutusScript(bytes)),
            PlaceholderScript::PlutusV2(bytes) => MemoizedScript::PlutusV2Script(PlutusScript(bytes)),
            PlaceholderScript::PlutusV3(bytes) => MemoizedScript::PlutusV3Script(PlutusScript(bytes)),
        })
    }
}
