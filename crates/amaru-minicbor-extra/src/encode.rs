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
use minicbor::{Encoder, data::Tag};
use num::One;
use num_bigint::BigInt;

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

pub fn encode_bigint<W: cbor::encode::Write>(
    e: &mut Encoder<W>,
    i: &BigInt,
) -> Result<(), cbor::encode::Error<W::Error>> {
    let (header, tag, bytes) = match i.sign() {
        num_bigint::Sign::NoSign => {
            e.u8(0)?;
            return Ok(());
        }
        num_bigint::Sign::Plus => (0x00, 2, i.to_bytes_be().1),
        num_bigint::Sign::Minus => (0x20, 3, ((-i) - BigInt::one()).to_bytes_be().1),
    };

    match bytes.len() {
        1 if bytes[0] <= 0x17 => {
            put(e, &[header | bytes[0]])?;
        }
        len @ 1..=8 => {
            let width = len.next_power_of_two();
            put(e, &[header | (24 + width.trailing_zeros() as u8)])?;
            let mut buf = [0u8; 8];
            buf[width - len..width].copy_from_slice(&bytes);
            put(e, &buf[..width])?;
        }
        _ => {
            e.tag(Tag::new(tag))?;
            encode_bytestring(e, &bytes)?;
        }
    }

    Ok(())
}

fn put<W: minicbor::encode::Write>(e: &mut Encoder<W>, bytes: &[u8]) -> Result<(), minicbor::encode::Error<W::Error>> {
    e.writer_mut().write_all(bytes).map_err(minicbor::encode::Error::write)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_bigint;

    #[test]
    fn encode_bigint_cases() {
        for (value, expected) in cases() {
            assert_eq!(encoded(value.clone()), expected, "encoding {value}");
        }
    }

    #[test]
    fn roundtrip_bigint_cases() {
        for (value, _) in cases() {
            assert_eq!(decoded(&encoded(value.clone())), value, "roundtrip {value}");
        }
    }

    // HELPERS

    /// Interesting values to exercise the encoder: zero, the small-integer boundary (±23),
    /// the native-integer boundary (±2^64), and arbitrary-precision bignums in both directions.
    /// Each value is paired with its expected canonical CBOR encoding.
    fn cases() -> Vec<(BigInt, &'static str)> {
        let two_64: BigInt = BigInt::from(1u8) << 64;
        vec![
            (BigInt::from(0), "00"),
            (BigInt::from(23), "17"),
            (BigInt::from(65536u32), "1a00010000"),
            (BigInt::from(-65537i32), "3a00010000"),
            (BigInt::from(4294967296u64), "1b0000000100000000"),
            (BigInt::from(281474976710656u64), "1b0001000000000000"),
            (&two_64 - BigInt::one(), "1bffffffffffffffff"),
            (two_64.clone(), "c249010000000000000000"),
            (big(), "c24c033b2e3c9fd0803ce7ffffff"),
            (-big(), "c34c033b2e3c9fd0803ce7fffffe"),
            (-two_64, "3bffffffffffffffff"),
            (BigInt::from(-23), "36"),
        ]
    }

    /// A very large BigInt: 999999999999999999999999999
    fn big() -> BigInt {
        BigInt::from_bytes_be(num_bigint::Sign::Plus, &hex::decode("033b2e3c9fd0803ce7ffffff").unwrap())
    }

    fn encoded(i: BigInt) -> String {
        let mut e = Encoder::new(Vec::new());
        encode_bigint(&mut e, &i).expect("failed to encode bigint");
        hex::encode(e.into_writer())
    }

    fn decoded(hex_str: &str) -> BigInt {
        let bytes = hex::decode(hex_str).unwrap();
        decode_bigint(&mut cbor::Decoder::new(&bytes)).expect("failed to decode bigint")
    }
}
