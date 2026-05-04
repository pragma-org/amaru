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

use amaru_uplc::{arena::Arena, binder::DeBruijn, flat, flat::FlatDecodeError, program::Program};
pub use pallas_primitives::conway::PlutusScript;

use crate::{HasMajorVersion, PlutusVersion, ProtocolVersion, ToBytes, cbor, reify_plutus_version};

/// Unwrap the CBOR bytestring envelope to get the raw flat-encoded program bytes.
impl<const V: usize> ToBytes for PlutusScript<V> {
    fn to_bytes(&self) -> Result<&[u8], cbor::decode::Error> {
        cbor::Decoder::new(&self.0).bytes()
    }
}

/// Decode a 'DeBruijn' UPLC program from encoded flat bytes.
pub fn decode_plutus_script<'a, const V: usize>(
    script: &PlutusScript<V>,
    protocol_version: ProtocolVersion,
    arena: &'a Arena,
) -> Result<(&'a Program<'a, DeBruijn>, PlutusVersion), FlatDecodeError> {
    let bytes = script.to_bytes().map_err(|e| {
        FlatDecodeError::Message(format!("unable to get raw flat bytes: error={e}, script={script:#?}"))
    })?;

    let pv = protocol_version.major();

    // TODO: carry IsKnownPlutusVersion constraint
    //
    // We should carry the `IsKnownPlutusVersion` constraint up until here if possible, so
    // that this conversion can be infaillible. This means that upon successfully decoding a
    // transaction, we should instantiate the constraint and carry it through; until that is too
    // cumbersome.
    let plutus_version = reify_plutus_version::<V>()
        .ok_or_else(|| FlatDecodeError::Message(format!("unable to reify type-level Plutus version '{V:#?}' ??!")))?;

    let program = match plutus_version {
        PlutusVersion::V3 => flat::decode_strict::<DeBruijn>(arena, bytes, plutus_version, pv),
        PlutusVersion::V1 | PlutusVersion::V2 => flat::decode::<DeBruijn>(arena, bytes, plutus_version, pv),
    }?;

    Ok((program, plutus_version))
}
