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

use std::{collections::BTreeMap, ops::Deref};

use crate::{AssetName, Hash, NonEmptyKeyValuePairs, cbor, size::SCRIPT};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Multiasset<A>(BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>);

impl<A> From<BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>> for Multiasset<A> {
    fn from(map: BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>) -> Self {
        Self(map)
    }
}

impl<A> Deref for Multiasset<A> {
    type Target = BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'d, C: cbor::HasProtocolVersion, A: for<'a> cbor::Decode<'a, C>> cbor::Decode<'d, C> for Multiasset<A> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.decode_with(ctx).map(Self)
    }
}

/// Write a map the way cardano-ledger does: definite-length up to 23 entries, indefinite-length
/// (header plus break byte) above.
impl<C, A: cbor::Encode<C>> cbor::Encode<C> for Multiasset<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        cbor::encode_variable_length_map(e, self.iter(), ctx)
    }
}
