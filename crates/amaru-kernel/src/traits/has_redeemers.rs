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

use crate::{ExUnits, PlutusData, RedeemerKey, Redeemers};
use std::{borrow::Cow, collections::BTreeMap};

pub trait HasRedeemers {
    fn redeemers(&self) -> BTreeMap<Cow<'_, RedeemerKey>, (&ExUnits, &PlutusData)>;
}

impl HasRedeemers for Redeemers {
    /// Flatten all redeemers kind into a map; This mimicks the Haskell's implementation and
    /// automatically perform de-duplication of redeemers.
    ///
    /// Indeed, it's possible that a list could have a (tag, index) tuple present more than once, with different data.
    /// The haskell node removes duplicates, keeping the last value present.
    ///
    /// See also <https://github.com/IntersectMBO/cardano-ledger/blob/607a7fdad352eb72041bb79f37bc1cf389432b1d/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L626>:
    ///
    /// - The Map.fromList behavior is documented here: <https://hackage.haskell.org/package/containers-0.6.6/docs/Data-Map-Strict.html#v:fromList>
    ///
    /// In this case, we don't care about the data provided in the redeemer (we're returning just the keys), so it doesn't matter.
    /// But this will come up during Phase 2 validation, so keep in mind that BTreeSet always keeps the first occurance based on the `PartialEq` result:
    ///
    /// <https://doc.rust-lang.org/std/collections/btree_set/struct.BTreeSet.html#method.insert>
    fn redeemers(&self) -> BTreeMap<Cow<'_, RedeemerKey>, (&ExUnits, &PlutusData)> {
        match self {
            Redeemers::List(list) => list
                .iter()
                .map(|redeemer| {
                    (
                        Cow::Owned(RedeemerKey {
                            tag: redeemer.tag,
                            index: redeemer.index,
                        }),
                        (&redeemer.ex_units, &redeemer.data),
                    )
                })
                .collect(),
            Redeemers::Map(map) => map
                .iter()
                .map(|(key, redeemer)| (Cow::Borrowed(key), (&redeemer.ex_units, &redeemer.data)))
                .collect(),
        }
    }
}
