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

//! The integer backend used by the UPLC runtime.
//!
//! The runtime only needs arbitrary-precision signed and unsigned integers plus a small set of
//! conversions. Keeping those operations here lets the UPLC crate select an implementation without
//! exposing backend-specific traits at evaluation call sites.

#[cfg(not(any(feature = "dashu", feature = "malachite")))]
compile_error!("one of the `dashu` or `malachite` features must be enabled");

// Cargo unifies features across the workspace. The release build explicitly adds
// `malachite` while dependencies still request the default `dashu` feature, so
// Malachite must take precedence when both selectors are active.

#[cfg(all(feature = "dashu", not(feature = "malachite")))]
pub type Integer = dashu_int::IBig;
#[cfg(all(feature = "dashu", not(feature = "malachite")))]
pub(crate) type Natural = dashu_int::UBig;

#[cfg(feature = "malachite")]
pub type Integer = malachite_nz::integer::Integer;
#[cfg(feature = "malachite")]
pub(crate) type Natural = malachite_nz::natural::Natural;

#[cfg(feature = "malachite")]
use malachite_base::num::arithmetic::traits::DivRem;

pub(crate) fn integer_zero() -> Integer {
    Integer::from(0u8)
}

pub(crate) fn natural_zero() -> Natural {
    Natural::from(0u8)
}

pub(crate) fn integer_is_negative(value: &Integer) -> bool {
    value < &integer_zero()
}

pub(crate) fn integer_is_zero(value: &Integer) -> bool {
    value == &Integer::from(0u8)
}

pub(crate) fn integer_gt_u8(value: &Integer, bound: u8) -> bool {
    value > &Integer::from(bound)
}

pub(crate) fn integer_gt_usize(value: &Integer, bound: usize) -> bool {
    value > &Integer::from(bound)
}

pub(crate) fn integer_gt_i64(value: &Integer, bound: i64) -> bool {
    value > &Integer::from(bound)
}

pub(crate) fn natural_is_one(value: &Natural) -> bool {
    value == &Natural::from(1u8)
}

pub(crate) fn integer_unsigned_abs(value: &Integer) -> Natural {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    use dashu_base::UnsignedAbs;
    #[cfg(feature = "malachite")]
    use malachite_base::num::arithmetic::traits::UnsignedAbs;

    value.clone().unsigned_abs()
}

pub(crate) fn natural_bit_len(value: &Natural) -> u64 {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    {
        use dashu_base::BitTest;

        value.bit_len() as u64
    }

    #[cfg(feature = "malachite")]
    {
        use malachite_base::num::logic::traits::SignificantBits;

        value.significant_bits()
    }
}

pub(crate) fn natural_bit(value: &Natural, index: u64) -> bool {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    {
        use dashu_base::BitTest;

        value.bit(index as usize)
    }

    #[cfg(feature = "malachite")]
    {
        use malachite_base::num::logic::traits::BitAccess;

        value.get_bit(index)
    }
}

pub(crate) fn natural_from_be_bytes(bytes: &[u8]) -> Natural {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    {
        Natural::from_be_bytes(bytes)
    }

    #[cfg(feature = "malachite")]
    {
        let mut value = natural_zero();
        for byte in bytes {
            value <<= 8usize;
            value += Natural::from(*byte);
        }
        value
    }
}

pub(crate) fn natural_to_be_bytes(value: &Natural) -> Vec<u8> {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    {
        value.to_be_bytes().into()
    }

    #[cfg(feature = "malachite")]
    {
        let mut value = value.clone();
        let base = Natural::from(256u16);
        let mut bytes = Vec::new();

        while value != natural_zero() {
            let byte = &value % &base;
            let byte = u8::try_from(&byte).unwrap_or_else(|_| unreachable!("base-256 remainder fits in a byte"));
            bytes.push(byte);
            value >>= 8usize;
        }

        bytes.reverse();
        bytes
    }
}

pub(crate) fn integer_bit_len(value: &Integer) -> u64 {
    natural_bit_len(&integer_unsigned_abs(value))
}

pub(crate) fn integer_from_bytes(bytes: &[u8], big_endian: bool) -> Integer {
    let natural = if big_endian {
        natural_from_be_bytes(bytes)
    } else {
        let mut reversed = bytes.to_vec();
        reversed.reverse();
        natural_from_be_bytes(&reversed)
    };

    integer_from_natural(natural)
}

pub(crate) fn integer_from_natural(natural: Natural) -> Integer {
    Integer::from(natural)
}

pub(crate) fn integer_to_bytes(value: &Integer, big_endian: bool) -> Vec<u8> {
    let mut bytes = natural_to_be_bytes(&integer_unsigned_abs(value));
    if !big_endian {
        bytes.reverse();
    }
    bytes
}

pub(crate) fn integer_to_usize(value: &Integer) -> Option<usize> {
    usize::try_from(value).ok()
}

pub(crate) fn integer_to_u8(value: &Integer) -> Option<u8> {
    u8::try_from(value).ok()
}

pub(crate) fn natural_to_u64(value: &Natural) -> Option<u64> {
    u64::try_from(value).ok()
}

pub(crate) fn natural_to_i64(value: &Natural) -> Option<i64> {
    i64::try_from(value).ok()
}

pub(crate) fn integer_div_rem(dividend: &Integer, divisor: &Integer) -> (Integer, Integer) {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    use dashu_base::DivRem;
    dividend.div_rem(divisor)
}

pub(crate) fn integer_rem_euclid(dividend: &Integer, divisor: &Integer) -> Integer {
    let (_, remainder) = integer_div_rem(dividend, divisor);
    if integer_is_negative(&remainder) {
        remainder + integer_from_natural(integer_unsigned_abs(divisor))
    } else {
        remainder
    }
}

pub(crate) fn integer_extended_gcd(first: &Integer, second: &Integer) -> (Natural, Integer, Integer) {
    let mut old_r = first.clone();
    let mut r = second.clone();
    let mut old_s = Integer::from(1u8);
    let mut s = integer_zero();
    let mut old_t = integer_zero();
    let mut t = Integer::from(1u8);

    while r != integer_zero() {
        let quotient = integer_div_rem(&old_r, &r).0;
        let next_r = old_r - &quotient * &r;
        old_r = r;
        r = next_r;

        let next_s = old_s - &quotient * &s;
        old_s = s;
        s = next_s;

        let next_t = old_t - quotient * &t;
        old_t = t;
        t = next_t;
    }

    (integer_unsigned_abs(&old_r), old_s, old_t)
}

pub(crate) mod cbor {
    use amaru_minicbor_extra::{assert_bounded_chunk, encode_bytestring};
    use minicbor::{
        Encoder,
        data::{IanaTag, Tag, Type},
        decode,
    };

    use super::{Integer, integer_from_bytes, integer_is_negative, integer_is_zero, integer_to_bytes};

    pub(crate) fn decode_integer(d: &mut minicbor::Decoder<'_>) -> Result<Integer, decode::Error> {
        decode_integer_with(d, |_| Ok(()))
    }

    pub(crate) fn decode_bounded_integer(d: &mut minicbor::Decoder<'_>) -> Result<Integer, decode::Error> {
        decode_integer_with(d, assert_bounded_chunk)
    }

    fn decode_integer_with(
        d: &mut minicbor::Decoder<'_>,
        check_chunk: impl Fn(&[u8]) -> Result<(), decode::Error>,
    ) -> Result<Integer, decode::Error> {
        if d.datatype()? == Type::Tag {
            let tag = d.tag()?;
            return match tag.try_into() {
                Ok(iana @ (IanaTag::PosBignum | IanaTag::NegBignum)) => {
                    let mut bytes = Vec::new();
                    for chunk in d.bytes_iter()? {
                        let chunk = chunk?;
                        check_chunk(chunk)?;
                        bytes.extend_from_slice(chunk);
                    }

                    let magnitude = integer_from_bytes(&bytes, true);
                    Ok(if iana == IanaTag::PosBignum { magnitude } else { -magnitude - Integer::from(1u8) })
                }
                _ => Err(decode::Error::message(format!("unexpected tag for bignum: {tag}"))),
            };
        }

        let i: i128 = d.int()?.into();
        Ok(Integer::from(i))
    }

    pub(crate) fn encode_integer<W: minicbor::encode::Write>(
        e: &mut Encoder<W>,
        integer: &Integer,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        if integer_is_zero(integer) {
            e.u8(0)?;
            return Ok(());
        }
        if integer == &Integer::from(-1i8) {
            e.i8(-1)?;
            return Ok(());
        }

        let negative = integer_is_negative(integer);
        let magnitude = if negative {
            integer_to_bytes(&(Integer::from(0u8) - integer - Integer::from(1u8)), true)
        } else {
            integer_to_bytes(integer, true)
        };
        let header = if negative { 0x20 } else { 0x00 };
        let tag = if negative { 3 } else { 2 };

        match magnitude.len() {
            1 if magnitude[0] <= 0x17 => put(e, &[header | magnitude[0]])?,
            len @ 1..=8 => {
                let width = len.next_power_of_two();
                put(e, &[header | (24 + width.trailing_zeros() as u8)])?;
                let mut buf = [0u8; 8];
                buf[width - len..width].copy_from_slice(&magnitude);
                put(e, &buf[..width])?;
            }
            _ => {
                e.tag(Tag::new(tag))?;
                encode_bytestring(e, &magnitude)?;
            }
        }

        Ok(())
    }

    fn put<W: minicbor::encode::Write>(
        e: &mut Encoder<W>,
        bytes: &[u8],
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(bytes).map_err(minicbor::encode::Error::write)
    }
}
