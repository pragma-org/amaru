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

use std::cmp::Ordering;

use crate::{CostModels, Hash, Hasher, Language, ProtocolParameters, WitnessSet, cbor, cbor::Encode};

/// A language-dependent view of protocol parameters used when computing the script integrity hash
/// (`script_data_hash` field on the transaction body).
///
/// Each Plutus language version used by a transaction contributes one `LanguageView`, which pairs
/// the language with its cost model parameters from the protocol parameters. These views are
/// CBOR-encoded and included in the script integrity hash.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LanguageView {
    pub language: Language,
    /// `None` when the protocol parameters carry no cost model for this language.
    /// Encoded as CBOR null (`0xf6`) — matches Haskell's `maybe encodeNull encodeCostModel`.
    pub cost_model: Option<Vec<i64>>,
}

/// Ordering follows the canonical CBOR "shortLex" rule on the encoded language tag:
/// shorter tags sort first, ties broken lexicographically. Concretely:
/// PlutusV2 (tag `0x01`, 1 byte) < PlutusV3 (tag `0x02`, 1 byte) < PlutusV1 (tag `0x41 0x00`, 2 bytes)
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/ca9b8c285e4493f2d25354914f8aae5483595507/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/PParams.hs#L587-L597>
impl Ord for LanguageView {
    fn cmp(&self, other: &Self) -> Ordering {
        let tag_a = self.encoded_tag();
        let tag_b = other.encoded_tag();
        tag_a.len().cmp(&tag_b.len()).then_with(|| tag_a.cmp(tag_b))
    }
}

impl PartialOrd for LanguageView {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl LanguageView {
    // TODO: hidden cloning of cost models
    //
    // This function should likely return a reference instead of cloning the cost models.
    // It is not satisfactory to also let the caller clone the whole `CostModels`, because
    // it may lead to cloning way more than necessary.
    pub fn from_cost_models(language: Language, cost_models: &CostModels) -> Self {
        let cost_model = match language {
            Language::PlutusV1 => cost_models.plutus_v1.clone(),
            Language::PlutusV2 => cost_models.plutus_v2.clone(),
            Language::PlutusV3 => cost_models.plutus_v3.clone(),
        };
        Self { language, cost_model }
    }

    fn encoded_tag(&self) -> &'static [u8] {
        match self.language {
            Language::PlutusV1 => &[0x41, 0x00],
            Language::PlutusV2 => &[0x01],
            Language::PlutusV3 => &[0x02],
        }
    }
}

/// Encodes a single key-value pair for the language views map.
///
/// The key is the language tag and the value is the cost model parameters list. See the
/// [`LanguageView`] documentation for the version-specific encoding rules.
///
/// Corresponds to the Haskell's `getLanguageView` + `encodeCostModel`:
/// <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/PParams.hs>
impl<C> cbor::Encode<C> for LanguageView {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(self.encoded_tag()).map_err(cbor::encode::Error::write)?;

        match (&self.language, self.cost_model.as_ref()) {
            // PlutusV1: the cost model params are encoded as an indefinite-length list,
            // then the result is wrapped in a CBOR bytestring.
            #[expect(clippy::expect_used)]
            (Language::PlutusV1, Some(params)) => {
                let mut inner = Vec::new();
                {
                    let mut sub = cbor::Encoder::new(&mut inner);
                    sub.begin_array().expect("infallible: writing to Vec");
                    for &param in params {
                        sub.i64(param).expect("infallible: writing to Vec");
                    }
                    sub.end().expect("infallible: writing to Vec");
                }
                e.bytes(&inner)?;
            }
            // PlutusV1 with no cost model: bytestring containing CBOR null, matching Haskell's
            // `serialize' version (serialize' version encodeNull)` double-bagging.
            (Language::PlutusV1, None) => {
                e.bytes(&[0xf6])?;
            }
            (Language::PlutusV2 | Language::PlutusV3, Some(params)) => {
                e.array(params.len() as u64)?;
                for &param in params {
                    e.i64(param)?;
                }
            }
            (Language::PlutusV2 | Language::PlutusV3, None) => {
                e.null()?;
            }
        };

        Ok(())
    }
}

/// Computes the script integrity hash from the witness set, protocol parameters, and the set of
/// Plutus languages used by the transaction. Returns `None` when there are no redeemers, datums,
/// or language views (matching Haskell's `SNothing` case).
///
/// The script integrity hash includes a canonical CBOR map of `{language_id => cost_model_params }`.
/// The encoding of this map has important version-specific quirks preserved for backward
/// compatibility:
///
/// **PlutusV1 (language id 0):**
/// - The cost model parameters are encoded as an **indefinite-length** CBOR list, then wrapped
///   in a CBOR bytestring. This was a bug in the original implementation that is now part of the
///   specification.
/// - The language id tag is **double-encoded**: first as a CBOR uint (0x00), then that encoding
///   is wrapped in a CBOR bytestring, producing `0x41 0x00`.
///
/// **PlutusV2 (language id 1) and PlutusV3 (language id 2):**
/// - The cost model parameters are encoded as a **definite-length** CBOR list.
/// - The language id tag is encoded normally as a single CBOR uint.
///
/// The language views map must be encoded canonically per RFC 7049 section 3.9:
/// - Definite-length encoding for maps, strings, and bytestrings
/// - Minimal integer encoding
/// - Keys sorted by length first (shorter before longer), then lexicographically
///
/// This means PlutusV2 (tag `0x01`, 1 byte) sorts before PlutusV1 (tag `0x41 0x00`, 2 bytes).
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/conway/impl/cddl/data/conway.cddl#L509>
pub fn compute_script_integrity_hash(
    witness_set: &WitnessSet,
    protocol_parameters: &ProtocolParameters,
    languages: &[Language],
) -> Option<Hash<32>> {
    let has_redeemers = witness_set.redeemer.is_some();
    let has_datums = witness_set.plutus_data.is_some();
    let has_languages = !languages.is_empty();

    if !has_redeemers && !has_datums && !has_languages {
        return None;
    }

    let mut buf = Vec::new();

    if let Some(ref redeemers) = witness_set.redeemer {
        buf.extend_from_slice(redeemers.original_bytes());
    } else {
        buf.push(0xa0);
    }

    if let Some(ref datums) = witness_set.plutus_data {
        buf.extend_from_slice(datums.original_bytes());
    }

    let mut views: Vec<LanguageView> = languages
        .iter()
        .map(|lang| LanguageView::from_cost_models(lang.clone(), &protocol_parameters.cost_models))
        .collect();

    views.sort();
    views.dedup();

    #[expect(clippy::expect_used)]
    {
        let mut e = cbor::Encoder::new(&mut buf);
        e.map(views.len() as u64).expect("infallible: writing to Vec");
        for view in &views {
            view.encode(&mut e, &mut ()).expect("infallible: writing to Vec");
        }
    }

    Some(Hasher::<256>::hash(&buf))
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

    #[test_case(Language::PlutusV1, &[0x41, 0x00, 0x41, 0xf6] ; "v1 null params")]
    #[test_case(Language::PlutusV2, &[0x01, 0xf6] ; "v2 null params")]
    #[test_case(Language::PlutusV3, &[0x02, 0xf6] ; "v3 null params")]
    fn language_view_encodes_null_when_cost_model_missing(language: Language, expected: &[u8]) {
        let view = LanguageView { language, cost_model: None };
        assert_eq!(crate::to_cbor(&view), expected);
    }
}
