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

use std::fmt;

use crate::{Hash, Hasher, Language, LanguageView, ProtocolParameters, WitnessSet, cbor, cbor::Encode};

/// All inputs that feed into the script integrity hash, kept around so a mismatch can be
/// reported with enough detail for a user to byte-diff against their own encoder.
///
/// The hash is `blake2b-256` of `redeemers_bytes || datums_bytes || cbor(language_views_map)`.
/// Each component is preserved here in the exact form that was hashed; `Display` renders them
/// as hex so the error message is small but reproducible.
///
/// See [`LanguageView`] for the version-specific encoding quirks of the language views map.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScriptIntegrityData {
    /// Either the original CBOR bytes of the redeemer set, or `[0xa0]` (an empty CBOR map)
    /// when the witness set has no redeemers but other inputs require a hash.
    redeemers_bytes: Vec<u8>,
    /// Original CBOR bytes of the plutus data set, or `None` when no datums are present.
    datums_bytes: Option<Vec<u8>>,
    /// Sorted, deduplicated language views.
    language_views: Vec<LanguageView>,
}

impl ScriptIntegrityData {
    /// Build the script integrity inputs from the witness set, protocol parameters, and the set
    /// of Plutus languages used by the transaction. Returns `None` when there are no redeemers,
    /// datums, or language views (matching Haskell's `SNothing` case).
    pub fn from_witness_set(
        witness_set: &WitnessSet,
        protocol_parameters: &ProtocolParameters,
        languages: &[Language],
    ) -> Option<Self> {
        let has_redeemers = witness_set.redeemer.is_some();
        let has_datums = witness_set.plutus_data.is_some();
        let has_languages = !languages.is_empty();

        if !has_redeemers && !has_datums && !has_languages {
            return None;
        }

        let redeemers_bytes = match witness_set.redeemer.as_ref() {
            Some(redeemers) => redeemers.original_bytes().to_vec(),
            None => vec![0xa0],
        };

        let datums_bytes = witness_set.plutus_data.as_ref().map(|d| d.original_bytes().to_vec());

        let mut language_views: Vec<LanguageView> = languages
            .iter()
            .map(|lang| LanguageView::from_cost_models(lang.clone(), &protocol_parameters.cost_models))
            .collect();
        language_views.sort();
        language_views.dedup();

        Some(Self { redeemers_bytes, datums_bytes, language_views })
    }

    /// The exact byte sequence fed to blake2b-256 to produce the script integrity hash.
    pub fn encoded_buffer(&self) -> Vec<u8> {
        let mut buf = self.redeemers_bytes.clone();
        if let Some(ref datums) = self.datums_bytes {
            buf.extend_from_slice(datums);
        }
        self.encode_language_views_into(&mut buf);
        buf
    }

    pub fn hash(&self) -> Hash<32> {
        Hasher::<256>::hash(&self.encoded_buffer())
    }

    fn encode_language_views_into(&self, buf: &mut Vec<u8>) {
        #[expect(clippy::expect_used)]
        {
            let mut e = cbor::Encoder::new(buf);
            e.map(self.language_views.len() as u64).expect("infallible: writing to Vec");
            for view in &self.language_views {
                view.encode(&mut e, &mut ()).expect("infallible: writing to Vec");
            }
        }
    }

    fn language_views_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.encode_language_views_into(&mut buf);
        buf
    }
}

impl fmt::Display for ScriptIntegrityData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "redeemers: 0x{}, datums: {}, language_views: 0x{}",
            hex::encode(&self.redeemers_bytes),
            self.datums_bytes.as_ref().map(|d| format!("0x{}", hex::encode(d))).unwrap_or_else(|| "none".to_string()),
            hex::encode(self.language_views_bytes()),
        )
    }
}

/// Computes the script integrity hash from the witness set, protocol parameters, and the set of
/// Plutus languages used by the transaction. Returns `None` when there are no redeemers, datums,
/// or language views (matching Haskell's `SNothing` case).
///
/// Convenience wrapper around [`ScriptIntegrityData::hash`] for callers that don't need the
/// underlying inputs for diagnostics.
pub fn compute_script_integrity_hash(
    witness_set: &WitnessSet,
    protocol_parameters: &ProtocolParameters,
    languages: &[Language],
) -> Option<Hash<32>> {
    ScriptIntegrityData::from_witness_set(witness_set, protocol_parameters, languages).map(|d| d.hash())
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::PREPROD_DEFAULT_PROTOCOL_PARAMETERS;

    /// Selects which Plutus cost models are present in the protocol parameters used by a test.
    #[derive(Clone, Copy)]
    enum Pp {
        AllCostModels,
        NoV1,
        NoV2,
        NoV3,
    }

    impl Pp {
        fn build(self) -> ProtocolParameters {
            let mut pp = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();
            match self {
                Pp::AllCostModels => {}
                Pp::NoV1 => pp.cost_models.plutus_v1 = None,
                Pp::NoV2 => pp.cost_models.plutus_v2 = None,
                Pp::NoV3 => pp.cost_models.plutus_v3 = None,
            }
            pp
        }
    }

    #[test_case(Pp::AllCostModels, &[], None ; "no inputs returns None")]
    #[test_case(Pp::AllCostModels, &[Language::PlutusV1], Some("d278610b21b4804243125c49fa820a691fa3dd79cf4109ade08090068f466750") ; "v1 only")]
    #[test_case(Pp::AllCostModels, &[Language::PlutusV2], Some("2a6094c211d2bfca5c6ae0c4f8ae3556db95345d94a53a309a2eabc8b8083843") ; "v2 only")]
    #[test_case(Pp::AllCostModels, &[Language::PlutusV3], Some("870bd0633099c971d633098389a3479c85328f1f36e97ab8ee99638019034707") ; "v3 only")]
    #[test_case(Pp::AllCostModels, &[Language::PlutusV1, Language::PlutusV2], Some("c570a138a1c2a043e1ef7091f4942458ca13ad19372189f877eb110a118dcac1") ; "v1 + v2")]
    #[test_case(Pp::AllCostModels, &[Language::PlutusV2, Language::PlutusV1], Some("c570a138a1c2a043e1ef7091f4942458ca13ad19372189f877eb110a118dcac1") ; "v2 + v1 sorts to v1 + v2")]
    #[test_case(Pp::AllCostModels, &[Language::PlutusV1, Language::PlutusV1], Some("d278610b21b4804243125c49fa820a691fa3dd79cf4109ade08090068f466750") ; "v1 + v1 dedups to v1")]
    #[test_case(Pp::NoV1, &[Language::PlutusV1], Some("ac871208ce24bcee5ca68ab58c321750fa80f73bfb4e5d9a6acc2b1e3817dcc4") ; "v1 only with no v1 cost model")]
    #[test_case(Pp::NoV2, &[Language::PlutusV2], Some("b96a45ab8016ded02c3fbf1939c695e86155a961c83fc695aba5316086ec47e3") ; "v2 only with no v2 cost model")]
    #[test_case(Pp::NoV3, &[Language::PlutusV3], Some("302569f5de0c5cd3d1816ff8b95a9117398227639a0425915ee4f68b590382a1") ; "v3 only with no v3 cost model")]
    fn integrity_hash(pp: Pp, languages: &[Language], expected: Option<&str>) {
        let actual = compute_script_integrity_hash(&WitnessSet::default(), &pp.build(), languages);
        let actual_hex = actual.map(|h| hex::encode(h.as_ref()));
        assert_eq!(actual_hex.as_deref(), expected);
    }

    #[test]
    fn display_renders_components_as_hex() {
        let pp = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();
        let data = ScriptIntegrityData::from_witness_set(&WitnessSet::default(), &pp, &[Language::PlutusV3])
            .expect("languages provided => Some");

        let rendered = data.to_string();
        assert!(rendered.contains("redeemers: 0xa0"), "redeemers should default to empty CBOR map: {rendered}");
        assert!(rendered.contains("datums: none"), "no datums in default witness set: {rendered}");
        assert!(rendered.contains("language_views: 0x"), "language_views section: {rendered}");
    }

    #[test]
    fn encoded_buffer_hashes_to_same_value() {
        let pp = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();
        let data = ScriptIntegrityData::from_witness_set(&WitnessSet::default(), &pp, &[Language::PlutusV2])
            .expect("languages provided => Some");

        assert_eq!(Hasher::<256>::hash(&data.encoded_buffer()), data.hash());
    }
}
