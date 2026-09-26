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

use std::{collections::BTreeMap, fmt};

use crate::{CostModel, cbor};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CostModels {
    pub plutus_v1: Option<CostModel>,

    pub plutus_v2: Option<CostModel>,

    pub plutus_v3: Option<CostModel>,

    /// Cost models for script languages this node does not know about.
    ///
    /// ```cddl
    /// cost_models = {? 0 : [* int64], ? 1 : [* int64], ? 2 : [* int64], * 3 .. 255 => [* int64]}
    /// ```
    ///
    /// The ledger keeps these verbatim (`costModelsUnknown`) rather than discarding them, so that a
    /// protocol parameter update introducing a future Plutus version applies identically here and on
    /// the Haskell node, and hashes taken over the parameters agree.
    #[serde(default)]
    pub unknown: BTreeMap<u8, CostModel>,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for CostModels {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_map_with_unique_keys(
            d,
            ctx,
            CostModels::default(),
            // Language ids are `3 .. 255` for unknown languages, so they never exceed a byte.
            |d, _ctx| d.u8(),
            |d, ctx, models, language| {
                let model: CostModel = d.decode_with(ctx)?;
                match language {
                    0 => models.plutus_v1 = Some(model),
                    1 => models.plutus_v2 = Some(model),
                    2 => models.plutus_v3 = Some(model),
                    _ => {
                        models.unknown.insert(language, model);
                    }
                }
                Ok(())
            },
        )
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for CostModels {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        // The known languages and the unknown ones form a single map, ordered by language id.
        let mut entries: Vec<(u8, &CostModel)> = [(0, &self.plutus_v1), (1, &self.plutus_v2), (2, &self.plutus_v3)]
            .into_iter()
            .filter_map(|(language, model)| model.as_ref().map(|model| (language, model)))
            .collect();
        entries.extend(self.unknown.iter().map(|(language, model)| (*language, model)));
        entries.sort_by_key(|(language, _)| *language);

        cbor::encode_variable_length_map(e, entries.iter().map(|(language, model)| (language, *model)), ctx)
    }
}

impl fmt::Display for CostModels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: destructuring for completeness static checks
        let CostModels { plutus_v1, plutus_v2, plutus_v3, unknown } = self;

        let mut needs_separator = false;

        if let Some(cost_model) = plutus_v1 {
            write!(f, "plutus_v1 = {:?}", cost_model)?;
            needs_separator = true;
        }

        if let Some(cost_model) = plutus_v2 {
            write!(f, "{}plutus_v2 = {:?}", if needs_separator { ", " } else { "" }, cost_model)?;
            needs_separator = true;
        }

        if let Some(cost_model) = plutus_v3 {
            write!(f, "{}plutus_v3 = {:?}", if needs_separator { ", " } else { "" }, cost_model)?;
            needs_separator = true;
        }

        for (key, cost_model) in unknown {
            write!(f, "{}unknown[{}] = {:?}", if needs_separator { ", " } else { "" }, key, cost_model)?;
            needs_separator = true;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::protocol_version::PROTOCOL_VERSION_10;

    /// From protocol version 9 the ledger assembles cost models with `decodeMap`, which refuses a
    /// map that repeats a key, whether the language is a known one or not.
    #[test_case(&[0xa1, 0x00, 0x81, 0x01]                         => matches Ok(_)  ; "one known language")]
    #[test_case(&[0xa2, 0x00, 0x81, 0x01, 0x01, 0x81, 0x02]       => matches Ok(_)  ; "two distinct languages")]
    #[test_case(&[0xa2, 0x00, 0x81, 0x01, 0x00, 0x81, 0x02]       => matches Err(_) ; "a known language twice")]
    #[test_case(&[0xa2, 0x18, 0x63, 0x80, 0x18, 0x63, 0x80]       => matches Err(_) ; "an unknown language twice")]
    fn decode_rejects_duplicate_languages(bytes: &[u8]) -> Result<CostModels, cbor::decode::Error> {
        let mut version = PROTOCOL_VERSION_10;
        cbor::from_cbor_no_leftovers_with(bytes, &mut version)
    }
}
