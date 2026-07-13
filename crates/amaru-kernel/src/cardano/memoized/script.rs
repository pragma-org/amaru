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

use pallas_codec::utils::CborWrap;
use pallas_primitives::{
    KeepRaw,
    conway::{NativeScript, PseudoScript},
};
use serde::ser::SerializeStruct;

use crate::{
    Bytes, ComputeHash, HasScriptHash, Hash, MemoizedNativeScript, PlutusScript, cbor,
    cbor::{bytes::ByteSlice, data::IanaTag},
    size::SCRIPT,
};

pub type MemoizedScript = PseudoScript<MemoizedNativeScript>;

/// A borrowed reference to a script.
///
/// The by-reference counterpart of the owned [`MemoizedScript`], flattened to its four
/// kinds: a native script, or a Plutus script whose language version is carried in the
/// type. The version travels with the script because execution depends on it.
/// The available builtins, the cost model, and the  script-context encoding all differ by Plutus version.
#[derive(Debug, Clone)]
pub enum BorrowedScript<'a> {
    Native(&'a NativeScript),
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
            MemoizedScript::NativeScript(script) => BorrowedScript::Native(script.as_ref()),
            MemoizedScript::PlutusV1Script(script) => BorrowedScript::PlutusV1(script),
            MemoizedScript::PlutusV2Script(script) => BorrowedScript::PlutusV2(script),
            MemoizedScript::PlutusV3Script(script) => BorrowedScript::PlutusV3(script),
        }
    }
}

impl HasScriptHash for BorrowedScript<'_> {
    fn script_hash(&self) -> Hash<SCRIPT> {
        match self {
            BorrowedScript::Native(native_script) => native_script.compute_hash(),
            BorrowedScript::PlutusV1(plutus_script) => plutus_script.compute_hash(),
            BorrowedScript::PlutusV2(plutus_script) => plutus_script.compute_hash(),
            BorrowedScript::PlutusV3(plutus_script) => plutus_script.compute_hash(),
        }
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

pub fn script_original_bytes(script: &MemoizedScript) -> &[u8] {
    match script {
        MemoizedScript::NativeScript(native) => native.original_bytes(),
        MemoizedScript::PlutusV1Script(plutus) => plutus.as_ref(),
        MemoizedScript::PlutusV2Script(plutus) => plutus.as_ref(),
        MemoizedScript::PlutusV3Script(plutus) => plutus.as_ref(),
    }
}

pub fn script_size(script: &MemoizedScript) -> u64 {
    script_original_bytes(script).len() as u64
}

pub fn decode_script<C>(d: &mut cbor::Decoder<'_>, ctx: &mut C) -> Result<MemoizedScript, cbor::decode::Error> {
    let tag = d.tag()?;
    if tag != IanaTag::Cbor.tag() {
        return Err(cbor::decode::Error::message(format!(
            "unexpected tag for script: expected {}, got {}",
            IanaTag::Cbor.tag(),
            tag
        )));
    }

    let script: PseudoScript<MemoizedNativeScript> = cbor::Decoder::new(d.bytes()?)
        .decode_with(ctx)
        .map_err(|e| cbor::decode::Error::message(format!("failed to decode script: {e}")))?;

    Ok(match script {
        PseudoScript::NativeScript(n) => MemoizedScript::NativeScript(n),
        PseudoScript::PlutusV1Script(s) => MemoizedScript::PlutusV1Script(s),
        PseudoScript::PlutusV2Script(s) => MemoizedScript::PlutusV2Script(s),
        PseudoScript::PlutusV3Script(s) => MemoizedScript::PlutusV3Script(s),
    })
}

pub fn encode_script<W: cbor::encode::Write>(
    script: &MemoizedScript,
    e: &mut cbor::Encoder<W>,
) -> Result<(), cbor::encode::Error<W::Error>> {
    e.tag(IanaTag::Cbor)?;

    let buffer = match script {
        MemoizedScript::NativeScript(native) => {
            let mut bytes = vec![
                130, // CBOR definite array of length 2
                0,   // Tag for Native Script
            ];
            bytes.extend_from_slice(native.original_bytes());
            bytes
        }
        MemoizedScript::PlutusV1Script(plutus) => {
            #[expect(clippy::unwrap_used)] // Infallible error.
            cbor::to_vec((1, Into::<&ByteSlice>::into(plutus.as_ref()))).unwrap()
        }
        MemoizedScript::PlutusV2Script(plutus) => {
            #[expect(clippy::unwrap_used)] // Infallible error.
            cbor::to_vec((2, Into::<&ByteSlice>::into(plutus.as_ref()))).unwrap()
        }
        MemoizedScript::PlutusV3Script(plutus) => {
            #[expect(clippy::unwrap_used)] // Infallible error.
            cbor::to_vec((3, Into::<&ByteSlice>::into(plutus.as_ref()))).unwrap()
        }
    };

    e.bytes(&buffer)?;

    Ok(())
}

pub fn from_minted_script(wrapper: CborWrap<PseudoScript<KeepRaw<'_, NativeScript>>>) -> MemoizedScript {
    match wrapper.0 {
        PseudoScript::NativeScript(script) => MemoizedScript::NativeScript(MemoizedNativeScript::from(script)),
        PseudoScript::PlutusV1Script(script) => MemoizedScript::PlutusV1Script(script),
        PseudoScript::PlutusV2Script(script) => MemoizedScript::PlutusV2Script(script),
        PseudoScript::PlutusV3Script(script) => MemoizedScript::PlutusV3Script(script),
    }
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

// --------------------------------------------------------------------- Helpers

#[derive(serde::Deserialize)]
pub(crate) enum PlaceholderScript {
    NativeScript(Bytes),
    PlutusV1(Bytes),
    PlutusV2(Bytes),
    PlutusV3(Bytes),
}
