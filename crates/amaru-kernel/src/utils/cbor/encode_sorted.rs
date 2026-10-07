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

use crate::{KeyValuePairs, NonEmptyKeyValuePairs, cbor};

/// Write a collection with its elements in ascending order.
///
/// The ledger holds such a field in a `Set` or a `Map`, which it re-encodes in key order — unlike a
/// field it holds in an `OSet` or replays from memoised bytes, which keeps the order it was read in.
/// Implementing this separately from [`cbor::Encode`] puts that choice at the field rather than in
/// the collection, so one collection type serves both.
pub trait EncodeSorted<C> {
    fn encode_sorted<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>>;
}

/// A set, written as a tagged array of ascending elements.
impl<C, E: Ord + cbor::Encode<C>> EncodeSorted<C> for [E] {
    fn encode_sorted<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::TAG_SET_258)?;

        let mut sorted = self.iter().collect::<Vec<_>>();
        sorted.sort_unstable();
        e.encode_with(sorted.as_slice(), ctx)?;

        Ok(())
    }
}

/// A map, written with its keys in ascending order.
///
/// NOTE: the keys of a map nested inside the values are not reached from here, so a nested map that
/// must also be written in key order sorts itself. See [`NonEmptyKeyValuePairs`].
impl<C, K: Ord + Eq + cbor::Encode<C>, V: cbor::Encode<C>> EncodeSorted<C> for KeyValuePairs<K, V> {
    fn encode_sorted<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let mut sorted = self.iter().collect::<Vec<_>>();
        sorted.sort_by_key(|(key, _)| key);
        cbor::encode_variable_length_map(e, sorted.into_iter().map(|(k, v)| (k, v)), ctx)
    }
}

impl<C, K: Ord + Eq + cbor::Encode<C>, V: cbor::Encode<C>> EncodeSorted<C> for NonEmptyKeyValuePairs<K, V> {
    fn encode_sorted<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let mut sorted = self.iter().collect::<Vec<_>>();
        sorted.sort_by_key(|(key, _)| key);
        cbor::encode_variable_length_map(e, sorted.into_iter().map(|(k, v)| (k, v)), ctx)
    }
}
