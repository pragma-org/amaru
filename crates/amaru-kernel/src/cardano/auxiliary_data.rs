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

use crate::{Hash, Hasher, KeyValuePairs, MemoizedNativeScript, Metadatum, NULL_HASH32, PlutusScript, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuxiliaryData {
    original_size: u64,

    hash: Hash<{ AuxiliaryData::HASH_SIZE }>,

    metadata: KeyValuePairs<u64, Metadatum>,

    native_scripts: Vec<MemoizedNativeScript>,

    plutus_v1_scripts: Vec<PlutusScript<1>>,

    plutus_v2_scripts: Vec<PlutusScript<2>>,

    plutus_v3_scripts: Vec<PlutusScript<3>>,
}

impl AuxiliaryData {
    /// Hash digest size, in bytes.
    pub const HASH_SIZE: usize = 32;

    /// Obtain the blake2b-256 hash digest of the serialised AuxiliaryData.
    pub fn hash(&self) -> Hash<{ Self::HASH_SIZE }> {
        self.hash
    }

    #[allow(clippy::len_without_is_empty)]
    /// Original size of the serialised bytes
    pub fn len(&self) -> u64 {
        self.original_size
    }

    /// Obtain the transaction metadata key-value pairs.
    pub fn metadata(&self) -> &KeyValuePairs<u64, Metadatum> {
        &self.metadata
    }

    /// Obtain the Plutus V1 scripts embedded in the auxiliary data.
    pub fn plutus_v1_scripts(&self) -> &[PlutusScript<1>] {
        &self.plutus_v1_scripts
    }

    /// Obtain the Plutus V2 scripts embedded in the auxiliary data.
    pub fn plutus_v2_scripts(&self) -> &[PlutusScript<2>] {
        &self.plutus_v2_scripts
    }

    /// Obtain the Plutus V3 scripts embedded in the auxiliary data.
    pub fn plutus_v3_scripts(&self) -> &[PlutusScript<3>] {
        &self.plutus_v3_scripts
    }
}

impl Default for AuxiliaryData {
    fn default() -> Self {
        Self {
            hash: NULL_HASH32,
            original_size: 0,
            metadata: KeyValuePairs::default(),
            native_scripts: Vec::default(),
            plutus_v1_scripts: Vec::default(),
            plutus_v2_scripts: Vec::default(),
            plutus_v3_scripts: Vec::default(),
        }
    }
}

// ```cddl
// auxiliary_data = metadata / auxiliary_data_array / auxiliary_data_map
//
// metadata = {* metadatum_label => metadatum}
//
// metadatum_label = uint .size 8
//
// auxiliary_data_array =
//   [ transaction_metadata : metadata
//   , auxiliary_scripts : auxiliary_scripts
//   ]
//
// auxiliary_scripts = [* native_script]
//
// auxiliary_data_map =
//   #6.259(
//     { ? 0 : metadata
//     , ? 1 : [* native_script]
//     , ? 2 : [* plutus_v1_script]
//     , ? 3 : [* plutus_v2_script]
//     , ? 4 : [* plutus_v3_script]
//     }
//
//   )
// ```
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
            any => Err(cbor::decode::Error::message(format!("unexpected type {any} when decoding auxiliary data"))),
        }?;

        let end_position = d.position();

        Ok(Self {
            hash: Hasher::<256>::hash(&original_bytes[start_position..end_position]),
            original_size: (end_position - start_position) as u64,
            ..aux_data
        })
    }
}

/// Auxiliary data is always re-encoded in the Conway form, whichever era's form it was decoded from,
/// with empty entries omitted.
impl<C> cbor::Encode<C> for AuxiliaryData {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::TAG_MAP_259)?;

        let present = [
            !self.metadata.is_empty(),
            !self.native_scripts.is_empty(),
            !self.plutus_v1_scripts.is_empty(),
            !self.plutus_v2_scripts.is_empty(),
            !self.plutus_v3_scripts.is_empty(),
        ];

        e.map(present.iter().filter(|is_present| **is_present).count() as u64)?;

        if present[0] {
            e.u8(0)?;
            e.encode_with(&self.metadata, ctx)?;
        }
        if present[1] {
            e.u8(1)?;
            e.encode_with(&self.native_scripts, ctx)?;
        }
        if present[2] {
            e.u8(2)?;
            e.encode_with(&self.plutus_v1_scripts, ctx)?;
        }
        if present[3] {
            e.u8(3)?;
            e.encode_with(&self.plutus_v2_scripts, ctx)?;
        }
        if present[4] {
            e.u8(4)?;
            e.encode_with(&self.plutus_v3_scripts, ctx)?;
        }

        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Internals
// ----------------------------------------------------------------------------

impl AuxiliaryData {
    /// Decode some auxiliary data using the Shelley-era codecs.
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_shelley<'b, C>(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let metadata = d.decode_with(ctx)?;
        Ok(Self { metadata, ..Self::default() })
    }

    /// Decode some auxiliary data using the Allegra-era codecs
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_allegra<'b, C>(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let metadata = d.decode_with(ctx)?;
            let native_scripts = d.decode_with(ctx)?;
            Ok(Self { metadata, native_scripts, ..Self::default() })
        })
    }

    /// Decode some auxiliary data using the Alonzo-era codecs
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_alonzo<'b, C>(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
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

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::AuxiliaryData;
    use crate::{Hasher, from_cbor_no_leftovers, to_cbor};

    // metadata = {721: 42}
    const METADATA: &str = "a11902d1182a";

    #[test_case("a11902d1182a", "d90103a100a11902d1182a" ; "shelley bare metadata map")]
    #[test_case("a0", "d90103a0" ; "shelley empty metadata map")]
    #[test_case("82a11902d1182a80", "d90103a100a11902d1182a" ; "allegra without scripts")]
    #[test_case("d90103a100a11902d1182a", "d90103a100a11902d1182a" ; "alonzo is unchanged")]
    #[test_case(
        "d90103a500a11902d1182a0180028003800480",
        "d90103a100a11902d1182a" ;
        "alonzo empty entries are omitted"
    )]
    #[test_case(
        "82a11902d1182a818200581c00000000000000000000000000000000000000000000000000000000",
        "d90103a200a11902d1182a01818200581c00000000000000000000000000000000000000000000000000000000" ;
        "allegra with a native script"
    )]
    fn encodes_as_conway(input: &str, expected: &str) {
        let aux: AuxiliaryData = from_cbor_no_leftovers(&hex::decode(input).unwrap()).unwrap();

        let encoded = to_cbor(&aux);
        assert_eq!(hex::encode(&encoded), expected, "unexpected encoding");

        let re_decoded: AuxiliaryData = from_cbor_no_leftovers(&encoded).unwrap();
        assert_eq!(to_cbor(&re_decoded), encoded, "encoding is not a fixed point");
    }

    #[test]
    fn hash_and_size_come_from_the_original_bytes() {
        let original = hex::decode(METADATA).unwrap();
        let aux: AuxiliaryData = from_cbor_no_leftovers(&original).unwrap();

        assert_eq!(aux.hash(), Hasher::<256>::hash(&original));
        assert_eq!(aux.len(), original.len() as u64);

        assert_ne!(aux.hash(), Hasher::<256>::hash(&to_cbor(&aux)));
    }
}
