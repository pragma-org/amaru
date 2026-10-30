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

use minicbor as cbor;
use minicbor::Encoder;

/// Wrap an iterator so it is encoded as an indefinite-length CBOR map.
pub struct IndefiniteMap<I>(pub I);

impl<C, I, K, V> cbor::Encode<C> for IndefiniteMap<I>
where
    I: Clone + IntoIterator<Item = (K, V)>,
    K: cbor::Encode<C>,
    V: cbor::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        encode_indefinite_map(e, self.0.clone(), ctx)
    }
}

/// Encode key-value pairs as an indefinite-length CBOR map.
pub fn encode_indefinite_map<C, K, V, W>(
    e: &mut Encoder<W>,
    values: impl IntoIterator<Item = (K, V)>,
    ctx: &mut C,
) -> Result<(), cbor::encode::Error<W::Error>>
where
    K: cbor::Encode<C>,
    V: cbor::Encode<C>,
    W: cbor::encode::Write,
{
    e.begin_map()?;
    for (key, value) in values {
        e.encode_with(&key, ctx)?;
        e.encode_with(&value, ctx)?;
    }
    e.end()?;
    Ok(())
}
