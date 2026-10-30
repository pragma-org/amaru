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

use std::convert::Infallible;

use minicbor as cbor;
use minicbor::Encoder;

pub mod indefinite_array;
pub mod indefinite_map;

pub use indefinite_array::{IndefiniteArray, encode_indefinite_array};
pub use indefinite_map::{IndefiniteMap, encode_indefinite_map};

/// Encode a field with an optional value.
pub fn encode_optional<C, T, W>(
    e: &mut cbor::Encoder<W>,
    ctx: &mut C,
    key: u8,
    value: &Option<T>,
) -> Result<(), cbor::encode::Error<W::Error>>
where
    T: cbor::Encode<C>,
    W: cbor::encode::Write,
{
    if let Some(value) = value {
        e.u8(key)?.encode_with(value, ctx)?;
    }

    Ok(())
}

pub fn encode_bytestring<'a, W: minicbor::encode::Write>(
    e: &'a mut Encoder<W>,
    bs: &[u8],
) -> Result<&'a mut Encoder<W>, minicbor::encode::Error<W::Error>> {
    const CHUNK_SIZE: usize = 64;

    if bs.len() <= 64 {
        e.bytes(bs)?;
    } else {
        e.begin_bytes()?;

        for b in bs.chunks(CHUNK_SIZE) {
            e.bytes(b)?;
        }

        e.end()?;
    }
    Ok(e)
}

/// Encode a map using a variable-length encoding; maps smaller than 24 elements are serialized as
/// definite maps, whereas larger maps uses indefinite maps to avoid encoding larger length.
pub fn encode_variable_length_map<'iter, C, K, V, W>(
    e: &mut Encoder<W>,
    map: impl ExactSizeIterator<Item = (&'iter K, &'iter V)>,
    ctx: &mut C,
) -> Result<(), minicbor::encode::Error<W::Error>>
where
    K: minicbor::Encode<C> + 'iter,
    V: minicbor::Encode<C> + 'iter,
    W: minicbor::encode::Write,
{
    let as_indef = map.len() > 23;

    if as_indef {
        e.begin_map()?;
    } else {
        e.map(map.len() as u64)?;
    }

    for (k, v) in map {
        e.encode_with(k, ctx)?;
        e.encode_with(v, ctx)?;
    }

    if as_indef {
        e.end()?;
    }

    Ok(())
}

/// Count serialized bytes for a given value, without allocating.
pub fn count_bytes<T: minicbor::Encode<()>>(value: &T) -> usize {
    let mut encoder = cbor::Encoder::new(ByteCounter::default());
    encoder.encode(value).unwrap_or_else(|_| unreachable!("writing to a ByteCounter cannot fail"));
    encoder.into_writer().length
}

/// A CBOR sink that only counts bytes
#[derive(Default)]
pub struct ByteCounter {
    pub length: usize,
}

impl cbor::encode::Write for ByteCounter {
    type Error = Infallible;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.length += buf.len();
        Ok(())
    }
}
