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

use crate::{
    Bytes, MemoizedNativeScript, PlutusScript, PseudoScript, cbor,
    cbor::{bytes::ByteSlice, data::IanaTag},
};
use serde::ser::SerializeStruct;

pub type MemoizedScript = PseudoScript<MemoizedNativeScript>;

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

impl TryFrom<PlaceholderScript> for MemoizedScript {
    type Error = String;

    fn try_from(placeholder: PlaceholderScript) -> Result<Self, Self::Error> {
        Ok(match placeholder {
            PlaceholderScript::NativeScript(bytes) => {
                MemoizedScript::NativeScript(MemoizedNativeScript::try_from(bytes)?)
            }
            // FIXME: We should at least verify that the inner bytes are _plausible_ Plutus
            // scripts. Not just gibberish. For V1, V2 and V3.
            PlaceholderScript::PlutusV1(bytes) => {
                MemoizedScript::PlutusV1Script(PlutusScript(bytes))
            }
            PlaceholderScript::PlutusV2(bytes) => {
                MemoizedScript::PlutusV2Script(PlutusScript(bytes))
            }
            PlaceholderScript::PlutusV3(bytes) => {
                MemoizedScript::PlutusV3Script(PlutusScript(bytes))
            }
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
