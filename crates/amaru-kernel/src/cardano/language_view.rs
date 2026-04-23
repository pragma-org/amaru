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

use crate::{CostModels, Language, cbor};

/// A language-dependent view of protocol parameters used when computing the script integrity hash
/// (`script_data_hash` field on the transaction body).
///
/// Each Plutus language version used by a transaction contributes one `LanguageView`, which pairs
/// the language with its cost model parameters from the protocol parameters. These views are
/// CBOR-encoded and included in the script integrity hash.
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
/// Reference: https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/conway/impl/cddl/data/conway.cddl#L509
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LanguageView {
    pub language: Language,
    pub cost_model: Vec<i64>,
}

/// Ordering follows the canonical CBOR "shortLex" rule on the encoded language tag:
/// shorter tags sort first, ties broken lexicographically. Concretely:
/// PlutusV2 (tag `0x01`, 1 byte) < PlutusV3 (tag `0x02`, 1 byte) < PlutusV1 (tag `0x41 0x00`, 2 bytes)
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
    pub fn from_cost_models(language: Language, cost_models: &CostModels) -> Option<Self> {
        let cost_model = match language {
            Language::PlutusV1 => cost_models.plutus_v1.clone()?,
            Language::PlutusV2 => cost_models.plutus_v2.clone()?,
            Language::PlutusV3 => cost_models.plutus_v3.clone()?,
        };
        Some(Self { language, cost_model })
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
/// https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/PParams.hs
impl<C> cbor::Encode<C> for LanguageView {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(self.encoded_tag()).map_err(cbor::encode::Error::write)?;

        match self.language {
            // PlutusV1: the cost model params are encoded as an indefinite-length list,
            // then the result is wrapped in a CBOR bytestring.
            #[expect(clippy::expect_used)]
            Language::PlutusV1 => {
                let mut inner = Vec::new();
                {
                    let mut sub = cbor::Encoder::new(&mut inner);
                    sub.begin_array().expect("infallible: writing to Vec");
                    for &param in &self.cost_model {
                        sub.i64(param).expect("infallible: writing to Vec");
                    }
                    sub.end().expect("infallible: writing to Vec");
                }
                e.bytes(&inner)?;
            }
            Language::PlutusV2 | Language::PlutusV3 => {
                e.array(self.cost_model.len() as u64)?;
                for &param in &self.cost_model {
                    e.i64(param)?;
                }
            }
        };

        Ok(())
    }
}
