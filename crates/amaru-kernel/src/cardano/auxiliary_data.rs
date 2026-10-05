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

use crate::{Hash, Hasher, KeyValuePairs, Metadatum, NativeScript, PlutusScript, cbor};

/// Transaction metadata, keyed by label.
pub type Metadata = KeyValuePairs<u64, Metadatum>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuxiliaryData {
    hash: Hash<{ AuxiliaryData::HASH_SIZE }>,

    original_size: u64,

    body: AuxiliaryDataBody,
}

/// AuxiliaryDataBody decodes auxiliary data in the era form it was encoded in, and re-encodes in the same form.
/// Re-encoding in the same form is only desirable to run the CBOR conformance tests, in order
/// to make valid comparisons with expected values.
///
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum AuxiliaryDataBody {
    Shelley {
        metadata: Metadata,
    },

    Allegra {
        metadata: Metadata,
        native_scripts: Vec<NativeScript>,
    },

    /// NOTE: strictly speaking this format appeared during the Alonzo era, but the
    /// plutus entries were introduced in the Babbage then the Conway eras.
    Conway {
        metadata: Option<Metadata>,
        native_scripts: Option<Vec<NativeScript>>,
        plutus_v1_scripts: Option<Vec<PlutusScript<1>>>,
        plutus_v2_scripts: Option<Vec<PlutusScript<2>>>,
        plutus_v3_scripts: Option<Vec<PlutusScript<3>>>,
    },
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

    /// Obtain the transaction metadata key-value pairs, when the value carries any.
    pub fn metadata(&self) -> Option<&Metadata> {
        match &self.body {
            AuxiliaryDataBody::Shelley { metadata } | AuxiliaryDataBody::Allegra { metadata, .. } => Some(metadata),
            AuxiliaryDataBody::Conway { metadata, .. } => metadata.as_ref(),
        }
    }

    /// Obtain the native scripts embedded in the auxiliary data.
    pub fn native_scripts(&self) -> &[NativeScript] {
        match &self.body {
            AuxiliaryDataBody::Shelley { .. } => &[],
            AuxiliaryDataBody::Allegra { native_scripts, .. } => native_scripts,
            AuxiliaryDataBody::Conway { native_scripts, .. } => native_scripts.as_deref().unwrap_or_default(),
        }
    }

    /// Obtain the Plutus V1 scripts embedded in the auxiliary data.
    pub fn plutus_v1_scripts(&self) -> &[PlutusScript<1>] {
        match &self.body {
            AuxiliaryDataBody::Conway { plutus_v1_scripts, .. } => plutus_v1_scripts.as_deref().unwrap_or_default(),
            AuxiliaryDataBody::Shelley { .. } | AuxiliaryDataBody::Allegra { .. } => &[],
        }
    }

    /// Obtain the Plutus V2 scripts embedded in the auxiliary data.
    pub fn plutus_v2_scripts(&self) -> &[PlutusScript<2>] {
        match &self.body {
            AuxiliaryDataBody::Conway { plutus_v2_scripts, .. } => plutus_v2_scripts.as_deref().unwrap_or_default(),
            AuxiliaryDataBody::Shelley { .. } | AuxiliaryDataBody::Allegra { .. } => &[],
        }
    }

    /// Obtain the Plutus V3 scripts embedded in the auxiliary data.
    pub fn plutus_v3_scripts(&self) -> &[PlutusScript<3>] {
        match &self.body {
            AuxiliaryDataBody::Conway { plutus_v3_scripts, .. } => plutus_v3_scripts.as_deref().unwrap_or_default(),
            AuxiliaryDataBody::Shelley { .. } | AuxiliaryDataBody::Allegra { .. } => &[],
        }
    }
}

/// The Conway form of auxiliary data carrying neither metadata nor scripts: `#6.259({})`.
const EMPTY_AUXILIARY_DATA: &[u8] = &[0xd9, 0x01, 0x03, 0xa0];

impl Default for AuxiliaryData {
    fn default() -> Self {
        Self {
            hash: Hasher::<256>::hash(EMPTY_AUXILIARY_DATA),
            original_size: EMPTY_AUXILIARY_DATA.len() as u64,
            body: AuxiliaryDataBody::Conway {
                metadata: None,
                native_scripts: None,
                plutus_v1_scripts: None,
                plutus_v2_scripts: None,
                plutus_v3_scripts: None,
            },
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
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for AuxiliaryData {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        use cbor::data::Type::*;

        let original_bytes = d.input();

        let start_position = d.position();

        #[allow(clippy::wildcard_enum_match_arm)]
        let body = match d.datatype()? {
            Map | MapIndef => Self::decode_shelley(d, ctx),
            Array | ArrayIndef => Self::decode_allegra(d, ctx),
            Tag => Self::decode_conway(d, ctx),
            any => Err(cbor::decode::Error::message(format!("unexpected type {any} when decoding auxiliary data"))),
        }?;

        let bytes = &original_bytes[start_position..d.position()];

        Ok(Self { hash: Hasher::<256>::hash(bytes), original_size: bytes.len() as u64, body })
    }
}

/// Auxiliary data re-encodes in the era form it was decoded from, with the same entries present.
///
/// Rewriting an older form into the Conway one would change the digest a transaction body commits
/// to, so each form has its own encoder.
impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for AuxiliaryData {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match &self.body {
            AuxiliaryDataBody::Shelley { metadata } => {
                e.encode_with(metadata, ctx)?;
            }

            AuxiliaryDataBody::Allegra { metadata, native_scripts } => {
                e.array(2)?;
                e.encode_with(metadata, ctx)?;
                e.encode_with(native_scripts, ctx)?;
            }

            AuxiliaryDataBody::Conway {
                metadata,
                native_scripts,
                plutus_v1_scripts,
                plutus_v2_scripts,
                plutus_v3_scripts,
            } => {
                e.tag(cbor::TAG_MAP_259)?;

                let present = [
                    metadata.is_some(),
                    native_scripts.is_some(),
                    plutus_v1_scripts.is_some(),
                    plutus_v2_scripts.is_some(),
                    plutus_v3_scripts.is_some(),
                ];
                e.map(present.iter().filter(|is_present| **is_present).count() as u64)?;

                if let Some(metadata) = metadata {
                    e.u8(0)?;
                    e.encode_with(metadata, ctx)?;
                }
                if let Some(native_scripts) = native_scripts {
                    e.u8(1)?;
                    e.encode_with(native_scripts, ctx)?;
                }
                if let Some(scripts) = plutus_v1_scripts {
                    e.u8(2)?;
                    e.encode_with(scripts, ctx)?;
                }
                if let Some(scripts) = plutus_v2_scripts {
                    e.u8(3)?;
                    e.encode_with(scripts, ctx)?;
                }
                if let Some(scripts) = plutus_v3_scripts {
                    e.u8(4)?;
                    e.encode_with(scripts, ctx)?;
                }
            }
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
    fn decode_shelley<'b, C: cbor::HasProtocolVersion>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<AuxiliaryDataBody, cbor::decode::Error> {
        Ok(AuxiliaryDataBody::Shelley { metadata: d.decode_with(ctx)? })
    }

    /// Decode some auxiliary data using the Allegra-era codecs
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_allegra<'b, C: cbor::HasProtocolVersion>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<AuxiliaryDataBody, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let metadata = d.decode_with(ctx)?;
            let native_scripts = d.decode_with(ctx)?;
            Ok(AuxiliaryDataBody::Allegra { metadata, native_scripts })
        })
    }

    /// Decode some auxiliary data using the Conway-era codecs
    ///
    /// /!\ Does not compute the underlying hash digest. This is a responsibility of the caller.
    fn decode_conway<'b, C: cbor::HasProtocolVersion>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<AuxiliaryDataBody, cbor::decode::Error> {
        if d.tag()? != cbor::TAG_MAP_259 {
            return Err(cbor::decode::Error::tag_mismatch(cbor::TAG_MAP_259));
        }

        let mut entries = ConwayEntries::default();

        cbor::heterogeneous_map_unique_keys(
            d,
            &mut entries,
            |d| d.u64(),
            |d, entries, k| {
                match k {
                    0 => entries.metadata = Some(d.decode_with(ctx)?),
                    1 => entries.native_scripts = Some(d.decode_with(ctx)?),
                    2 => entries.plutus_v1_scripts = Some(d.decode_with(ctx)?),
                    3 => entries.plutus_v2_scripts = Some(d.decode_with(ctx)?),
                    4 => entries.plutus_v3_scripts = Some(d.decode_with(ctx)?),
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

        Ok(AuxiliaryDataBody::Conway {
            metadata: entries.metadata,
            native_scripts: entries.native_scripts,
            plutus_v1_scripts: entries.plutus_v1_scripts,
            plutus_v2_scripts: entries.plutus_v2_scripts,
            plutus_v3_scripts: entries.plutus_v3_scripts,
        })
    }
}

/// The keys seen while decoding the Conway form, before they become a [`AuxiliaryDataBody::Conway`].
#[derive(Default)]
struct ConwayEntries {
    metadata: Option<Metadata>,
    native_scripts: Option<Vec<NativeScript>>,
    plutus_v1_scripts: Option<Vec<PlutusScript<1>>>,
    plutus_v2_scripts: Option<Vec<PlutusScript<2>>>,
    plutus_v3_scripts: Option<Vec<PlutusScript<3>>>,
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::{AuxiliaryData, EMPTY_AUXILIARY_DATA};
    use crate::{Hasher, from_cbor_no_leftovers, to_cbor};

    // metadata = {721: 42}
    const METADATA: &str = "a11902d1182a";

    #[test_case("a11902d1182a" ; "shelley bare metadata map")]
    #[test_case("a0" ; "shelley empty metadata map")]
    #[test_case("82a11902d1182a80" ; "allegra without scripts")]
    #[test_case("82a080" ; "allegra with neither metadata nor scripts")]
    #[test_case("d90103a100a11902d1182a" ; "conway")]
    #[test_case("d90103a500a11902d1182a0180028003800480" ; "conway keeps entries that are present but empty")]
    #[test_case("d90103a3028003800480" ; "conway omits the keys the writer left out")]
    #[test_case(
        "82a11902d1182a818200581c00000000000000000000000000000000000000000000000000000000" ;
        "allegra with a native script"
    )]
    fn re_encodes_in_the_form_it_was_decoded_from(input: &str) {
        let bytes = hex::decode(input).unwrap();
        let aux: AuxiliaryData = from_cbor_no_leftovers(&bytes).unwrap();

        assert_eq!(hex::encode(to_cbor(&aux)), input, "the era's own form must survive a round-trip");
    }

    #[test]
    fn hash_and_size_come_from_the_original_bytes() {
        let original = hex::decode(METADATA).unwrap();
        let aux: AuxiliaryData = from_cbor_no_leftovers(&original).unwrap();

        assert_eq!(aux.hash(), Hasher::<256>::hash(&original));
        assert_eq!(aux.len(), original.len() as u64);
    }

    /// A default value still has to encode to something a decoder accepts.
    #[test]
    fn default_is_the_empty_conway_form() {
        let aux = AuxiliaryData::default();

        assert_eq!(to_cbor(&aux), EMPTY_AUXILIARY_DATA);

        let re_decoded: AuxiliaryData = from_cbor_no_leftovers(EMPTY_AUXILIARY_DATA).unwrap();
        assert_eq!(re_decoded, aux);
    }

    /// Only the Conway form of auxiliary data carries Plutus scripts, and only Allegra onwards carries native ones.
    #[test]
    fn each_form_exposes_only_what_it_can_carry() {
        let shelley: AuxiliaryData = from_cbor_no_leftovers(&hex::decode(METADATA).unwrap()).unwrap();
        assert_eq!(shelley.metadata().map(|m| m.len()), Some(1));
        assert!(shelley.native_scripts().is_empty());
        assert!(shelley.plutus_v3_scripts().is_empty());

        let allegra: AuxiliaryData = from_cbor_no_leftovers(
            &hex::decode("82a11902d1182a818200581c00000000000000000000000000000000000000000000000000000000").unwrap(),
        )
        .unwrap();
        assert_eq!(allegra.native_scripts().len(), 1);
        assert!(allegra.plutus_v1_scripts().is_empty());
    }
}
