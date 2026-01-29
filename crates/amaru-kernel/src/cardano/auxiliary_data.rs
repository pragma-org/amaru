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

use crate::{
    Hash, Hasher, KeyValuePairs, MemoizedNativeScript, Metadatum, NULL_HASH32, PlutusScript, cbor,
};

#[derive(Debug, Clone, PartialEq, Eq, cbor::Encode, serde::Serialize, serde::Deserialize)]
#[cbor(map)]
pub struct AuxiliaryData {
    #[cbor(skip)]
    hash: Hash<{ AuxiliaryData::HASH_SIZE }>,

    #[n(0)]
    metadata: KeyValuePairs<u32, Metadatum>,

    #[n(1)]
    native_scripts: Vec<MemoizedNativeScript>,

    #[n(2)]
    plutus_v1_scripts: Vec<PlutusScript<1>>,

    #[n(3)]
    plutus_v2_scripts: Vec<PlutusScript<2>>,

    #[n(4)]
    plutus_v3_scripts: Vec<PlutusScript<3>>,
}

impl AuxiliaryData {
    /// Hash digest size, in bytes.
    pub const HASH_SIZE: usize = 32;

    /// Obtain the blake2b-256 hash digest of the serialised AuxiliaryData.
    pub fn hash(&self) -> Hash<{ Self::HASH_SIZE }> {
        self.hash
    }
}

impl Default for AuxiliaryData {
    fn default() -> Self {
        Self {
            hash: NULL_HASH32,
            metadata: KeyValuePairs::default(),
            native_scripts: Vec::default(),
            plutus_v1_scripts: Vec::default(),
            plutus_v2_scripts: Vec::default(),
            plutus_v3_scripts: Vec::default(),
        }
    }
}

/// FIXME: Multi-era decoding
impl<'b, C> cbor::Decode<'b, C> for AuxiliaryData {
    // NOTE: AuxiliaryData post-Alonzo decoding
    //
    // Even when decoding post-Alonzo auxiliary data, the choice of decoder is determined
    // dynamically based on the received format. Said differently, the Conway era decoding is
    // backward-compatible, unlike many other data-types.
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        use cbor::data::Type::*;

        let original_bytes = d.input();

        let start_position = d.position();

        #[allow(clippy::wildcard_enum_match_arm)]
        let aux_data = match d.datatype()? {
            Map | MapIndef => Self::decode_shelley(d, ctx),
            Array | ArrayIndef => Self::decode_allegra(d, ctx),
            Tag => Self::decode_alonzo(d, ctx),
            any => Err(cbor::decode::Error::message(format!(
                "unexpected type {any} when decoding auxiliary data"
            ))),
        }?;

        let end_position = d.position();

        Ok(Self {
            hash: Hasher::<256>::hash(&original_bytes[start_position..end_position]),
            ..aux_data
        })
    }
}

// ----------------------------------------------------------------------------
// Internals
// ----------------------------------------------------------------------------

impl AuxiliaryData {
    /// Decode some auxiliary data using the Shelley-era codecs.
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_shelley<'b, C>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<Self, cbor::decode::Error> {
        let metadata = d.decode_with(ctx)?;
        Ok(Self {
            metadata,
            ..Self::default()
        })
    }

    /// Decode some auxiliary data using the Allegra-era codecs
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_allegra<'b, C>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<Self, cbor::decode::Error> {
        let metadata = d.decode_with(ctx)?;
        let native_scripts = d.decode_with(ctx)?;
        Ok(Self {
            metadata,
            native_scripts,
            ..Self::default()
        })
    }

    /// Decode some auxiliary data using the Alonzo-era codecs
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_alonzo<'b, C>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<Self, cbor::decode::Error> {
        if d.tag()? != cbor::TAG_MAP_259 {
            return Err(cbor::decode::Error::tag_mismatch(cbor::TAG_MAP_259));
        }

        let mut st = Self::default();

        cbor::heterogeneous_map(
            d,
            &mut st,
            |d| d.u64(),
            |d, st, k| {
                match k {
                    0 => st.metadata = d.decode_with(ctx)?,
                    1 => st.native_scripts = d.decode_with(ctx)?,
                    2 => st.plutus_v1_scripts = d.decode_with(ctx)?,
                    3 => st.plutus_v2_scripts = d.decode_with(ctx)?,
                    4 => st.plutus_v3_scripts = d.decode_with(ctx)?,
                    _ => {
                        return Err(cbor::decode::Error::message(format!(
                            "unexpected field key {k} in auxiliary data"
                        ))
                        .at(d.position()));
                    }
                };

                Ok(())
            },
        )?;

        Ok(st)
    }
}
