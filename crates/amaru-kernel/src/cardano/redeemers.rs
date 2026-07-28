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

use std::{borrow::Cow, collections::BTreeMap, ops::Deref};

use crate::{
    BorrowedScript, Bytes, ExUnits, PlutusData, Redeemer, RedeemerKey, RedeemerValue, ScriptPurpose, cbor,
    utils::serde::SerdeUsingCbor,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(into = "SerdeUsingCbor<Self>", from = "SerdeUsingCbor<Self>")]
pub struct Redeemers {
    original_bytes: Bytes,
    inner: RedeemersInner,
}

impl From<SerdeUsingCbor<Redeemers>> for Redeemers {
    fn from(SerdeUsingCbor(redeemers): SerdeUsingCbor<Self>) -> Self {
        redeemers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RedeemersInner {
    Array(Vec<Redeemer>),
    Map(BTreeMap<RedeemerKey, RedeemerValue>),
}

impl<C> cbor::Encode<C> for Redeemers {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(self.original_bytes()).map_err(cbor::encode::Error::write)
    }
}

impl<'b, C> cbor::Decode<'b, C> for Redeemers {
    #[expect(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (inner, bytes) = cbor::tee(d, |d| match d.datatype()? {
            minicbor::data::Type::Array | minicbor::data::Type::ArrayIndef => {
                Ok(RedeemersInner::Array(d.decode_with(ctx)?))
            }
            minicbor::data::Type::Map | minicbor::data::Type::MapIndef => Ok(RedeemersInner::Map(d.decode_with(ctx)?)),
            _ => Err(minicbor::decode::Error::message("invalid type for redeemers struct")),
        })?;

        Ok(Self { original_bytes: Bytes::from(bytes.to_vec()), inner })
    }
}

impl Redeemers {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    /// Stream the deduplicated redeemers without forcing the caller to materialize the full
    /// [`BTreeMap`]. Dedup semantics match Haskell's `Map.fromList`: last value wins.
    pub fn iter_unique(&self) -> Box<dyn Iterator<Item = (Cow<'_, RedeemerKey>, &ExUnits, &PlutusData)> + '_> {
        // Flatten all redeemers kind into a map; This mimicks the Haskell's implementation and
        // automatically perform de-duplication of redeemers.
        //
        // Indeed, it's possible that a list could have a (tag, index) tuple present more than once, with different data.
        // The haskell node removes duplicates, keeping the last value present.
        //
        // See also <https://github.com/IntersectMBO/cardano-ledger/blob/607a7fdad352eb72041bb79f37bc1cf389432b1d/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L626>:
        //
        // - The Map.fromList behavior is documented here: <https://hackage.haskell.org/package/containers-0.6.6/docs/Data-Map-Strict.html#v:fromList>
        //
        // In this case, we don't care about the data provided in the redeemer (we're returning just the keys), so it doesn't matter.
        // But this will come up during Phase 2 validation, so keep in mind that BTreeSet always keeps the first occurance based on the `PartialEq` result:
        //
        // <https://doc.rust-lang.org/std/collections/btree_set/struct.BTreeSet.html#method.insert>
        match &self.inner {
            RedeemersInner::Array(array) => Box::new(
                array
                    .iter()
                    .map(|redeemer| {
                        (
                            Cow::Owned(RedeemerKey { tag: redeemer.tag, index: redeemer.index }),
                            (&redeemer.ex_units, &redeemer.data),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(k, (ex, data))| (k, ex, data)),
            ),

            RedeemersInner::Map(map) => {
                Box::new(map.iter().map(|(key, redeemer)| (Cow::Borrowed(key), &redeemer.ex_units, &redeemer.data)))
            }
        }
    }
}

/// A redeemer resolved against the transaction and UTxO set.
///
/// An on-chain redeemer carries only a pointer ([`RedeemerKey`]: tag + index)
/// alongside its `data` and `ex_units`. Resolving that pointer yields the concrete
/// [`ScriptPurpose`] it acts on and the [`BorrowedScript`] it dispatches to, both captured
/// in a `RedeemerEntry`. Each entry is therefore the unit of a single Plutus script execution.
#[derive(Debug)]
pub struct RedeemerEntry<'a> {
    pub purpose: ScriptPurpose<'a>,
    pub data: &'a PlutusData,
    pub ex_units: ExUnits,
    pub script: BorrowedScript<'a>,
}

/// A transaction's redeemers, each resolved and indexed by its pointer.
///
/// Maps every [`RedeemerKey`] (tag + index) to its [`RedeemerEntry`].
/// Unlike [`Redeemers`], which may arrive as either a list or a map,
/// this is always a deduplicated map: duplicate keys collapse last-wins, matching the ledger's `Map.fromList` semantics.
#[derive(Debug)]
pub struct PlutusRedeemers<'a>(BTreeMap<RedeemerKey, RedeemerEntry<'a>>);

impl<'a> PlutusRedeemers<'a> {
    pub fn new(inner: BTreeMap<RedeemerKey, RedeemerEntry<'a>>) -> Self {
        Self(inner)
    }
}

impl<'a> Deref for PlutusRedeemers<'a> {
    type Target = BTreeMap<RedeemerKey, RedeemerEntry<'a>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PlutusRedeemers<'_> {
    pub fn iter_from<'a>(
        redeemers: &'a Redeemers,
    ) -> Box<dyn Iterator<Item = (RedeemerKey, &'a PlutusData, ExUnits)> + 'a> {
        match &redeemers.inner {
            RedeemersInner::Array(array) => {
                Box::new(array.iter().map(|r| (RedeemerKey { tag: r.tag, index: r.index }, &r.data, r.ex_units)))
            }
            RedeemersInner::Map(map) => {
                Box::new(map.iter().map(|(key, value)| (key.clone(), &value.data, value.ex_units)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Redeemer, RedeemerTag, empty_bytes};

    #[test]
    fn iter_from_into_btreemap_keeps_last_for_duplicate_redeemers() {
        // Pins the property the bug fix is for: when two redeemers share (tag, index) but
        // carry different data/ex_units, the last occurrence wins, matching Haskell's
        // Map.fromList. The property holds because RedeemerKey is primitive: BTreeMap::insert
        // on Ord-equal keys replaces the value, and the value carries everything that varies.
        let make_redeemer = |mem: u64, steps: u64, payload: u8| Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::BoundedBytes(vec![payload].into()),
            ex_units: ExUnits { mem, steps },
        };

        let r1 = make_redeemer(100, 200, 0xAA);
        let r2 = make_redeemer(999, 888, 0xBB);

        let redeemers = Redeemers { original_bytes: empty_bytes(), inner: RedeemersInner::Array(vec![r1, r2.clone()]) };

        let map: BTreeMap<RedeemerKey, (&PlutusData, ExUnits)> =
            PlutusRedeemers::iter_from(&redeemers).map(|(k, data, ex_units)| (k, (data, ex_units))).collect();

        assert_eq!(map.len(), 1, "duplicate (tag, index) should collapse to one entry");

        let (key, (data, ex_units)) = map.iter().next().unwrap();
        assert_eq!(key.tag, RedeemerTag::Spend);
        assert_eq!(key.index, 0);
        assert_eq!(*ex_units, r2.ex_units, "last redeemer's ex_units must win");
        assert_eq!(**data, r2.data, "last redeemer's data must win");
    }
}
