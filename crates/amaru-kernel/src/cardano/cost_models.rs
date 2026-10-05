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

#[cfg(any(test, feature = "test-utils"))]
use proptest::{
    collection, option,
    prelude::{Arbitrary, BoxedStrategy, Strategy, any},
};

use crate::{CostModel, PlutusVersion, cbor};

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

impl CostModels {
    /// Apply an update on top of these cost models.
    ///
    /// A language carried by the update replaces the model currently held for it; languages absent
    /// from the update keep their model. Unknown languages are merged with the update taking
    /// precedence, and an unknown entry is dropped once a model for that language is held as a
    /// known one, so that a node which has learned a new Plutus version ends up with the same cost
    /// models as one which has not.
    pub fn update(&mut self, update: CostModels) {
        let CostModels { plutus_v1, plutus_v2, plutus_v3, unknown } = update;

        // NOTE: the exhaustive match is here so that adding a Plutus version fails to compile
        // rather than silently skipping that language's cost model update.
        match PlutusVersion::V1 {
            PlutusVersion::V1 | PlutusVersion::V2 | PlutusVersion::V3 => {
                if let Some(cost_model) = plutus_v1 {
                    self.plutus_v1 = Some(cost_model);
                }
                if let Some(cost_model) = plutus_v2 {
                    self.plutus_v2 = Some(cost_model);
                }
                if let Some(cost_model) = plutus_v3 {
                    self.plutus_v3 = Some(cost_model);
                }
            }
        }

        self.unknown.extend(unknown);

        for (language, cost_model) in [(0, &self.plutus_v1), (1, &self.plutus_v2), (2, &self.plutus_v3)] {
            if cost_model.is_some() {
                self.unknown.remove(&language);
            }
        }
    }
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

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for CostModels {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        let any_cost_model =
            || any::<[Option<i64>; 3]>().prop_map(|costs| costs.into_iter().flatten().collect::<CostModel>());

        (
            option::of(any_cost_model()),
            option::of(any_cost_model()),
            option::of(any_cost_model()),
            collection::btree_map(3u8..=u8::MAX, any_cost_model(), 0..3),
        )
            .prop_map(|(plutus_v1, plutus_v2, plutus_v3, unknown)| CostModels {
                plutus_v1,
                plutus_v2,
                plutus_v3,
                unknown,
            })
            .boxed()
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

    fn cost_model(value: i64) -> CostModel {
        [value].into_iter().collect()
    }

    #[test]
    fn update_replaces_only_the_languages_it_carries() {
        let mut models = CostModels {
            plutus_v1: Some(cost_model(1)),
            plutus_v2: Some(cost_model(2)),
            plutus_v3: None,
            unknown: BTreeMap::new(),
        };

        models.update(CostModels { plutus_v2: Some(cost_model(20)), ..CostModels::default() });

        assert_eq!(models.plutus_v1, Some(cost_model(1)), "an absent language keeps its model");
        assert_eq!(models.plutus_v2, Some(cost_model(20)), "a carried language is replaced");
        assert_eq!(models.plutus_v3, None, "an absent language stays absent");
    }

    #[test]
    fn update_merges_unknown_languages_and_wins_on_conflict() {
        let mut models =
            CostModels { unknown: BTreeMap::from([(3, cost_model(3)), (4, cost_model(4))]), ..CostModels::default() };

        models.update(CostModels {
            unknown: BTreeMap::from([(4, cost_model(40)), (5, cost_model(5))]),
            ..CostModels::default()
        });

        assert_eq!(
            models.unknown,
            BTreeMap::from([(3, cost_model(3)), (4, cost_model(40)), (5, cost_model(5))]),
            "the update wins on a shared language, the rest are merged"
        );
    }

    /// A node that has learned a Plutus version reads its model as a known one, while a node that
    /// has not keeps it under `unknown`. Dropping the stale entry is what keeps the two in step.
    #[test]
    fn update_drops_an_unknown_entry_once_the_language_is_known() {
        let mut models = CostModels { unknown: BTreeMap::from([(2, cost_model(99))]), ..CostModels::default() };

        models.update(CostModels { plutus_v3: Some(cost_model(3)), ..CostModels::default() });

        assert_eq!(models.plutus_v3, Some(cost_model(3)));
        assert!(models.unknown.is_empty(), "the entry for a now-known language is dropped");
    }
}
