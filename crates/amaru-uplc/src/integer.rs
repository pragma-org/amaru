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

use std::{
    fmt,
    ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Neg, Rem, Shl, ShlAssign, Shr, ShrAssign, Sub},
    str::FromStr,
};

#[cfg(not(any(feature = "dashu", feature = "malachite")))]
compile_error!("one of the `dashu` or `malachite` features must be enabled");

// Cargo unifies features across the workspace. The release build explicitly adds
// `malachite` while dependencies still request the default `dashu` feature, so
// Malachite must take precedence when both selectors are active.

#[cfg(all(feature = "dashu", not(feature = "malachite")))]
type BackendInteger = dashu_int::IBig;
#[cfg(feature = "malachite")]
type BackendInteger = malachite_nz::integer::Integer;

#[cfg(all(feature = "dashu", not(feature = "malachite")))]
type BackendNatural = dashu_int::UBig;
#[cfg(feature = "malachite")]
type BackendNatural = malachite_nz::natural::Natural;

#[cfg(feature = "malachite")]
use malachite_base::num::arithmetic::traits::DivRem;

#[repr(transparent)]
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Integer(BackendInteger);

#[repr(transparent)]
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Natural(BackendNatural);

impl Integer {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    pub const ZERO: Self = Self(dashu_int::IBig::ZERO);

    #[cfg(feature = "malachite")]
    pub const ZERO: Self = Self(<BackendInteger as malachite_base::num::basic::traits::Zero>::ZERO);

    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    pub const ONE: Self = Self(dashu_int::IBig::ONE);

    #[cfg(feature = "malachite")]
    pub const ONE: Self = Self(<BackendInteger as malachite_base::num::basic::traits::One>::ONE);
}

impl Natural {
    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    pub const ZERO: Self = Self(dashu_int::UBig::ZERO);

    #[cfg(feature = "malachite")]
    pub const ZERO: Self = Self(<BackendNatural as malachite_base::num::basic::traits::Zero>::ZERO);

    #[cfg(all(feature = "dashu", not(feature = "malachite")))]
    pub const ONE: Self = Self(dashu_int::UBig::ONE);

    #[cfg(feature = "malachite")]
    pub const ONE: Self = Self(<BackendNatural as malachite_base::num::basic::traits::One>::ONE);
}

macro_rules! impl_from {
    ($wrapper:ident, $backend:ident: $($source:ty),+ $(,)?) => {
        $(
            impl From<$source> for $wrapper {
                fn from(value: $source) -> Self {
                    Self($backend::from(value))
                }
            }
        )+
    };
}

impl_from!(Integer, BackendInteger: i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);
impl_from!(Natural, BackendNatural: u8, u16, u32, u64, u128, usize);

impl From<Natural> for Integer {
    fn from(value: Natural) -> Self {
        Self(BackendInteger::from(value.0))
    }
}

impl FromStr for Integer {
    type Err = <BackendInteger as FromStr>::Err;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        BackendInteger::from_str(value).map(Self)
    }
}

impl fmt::Display for Integer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for Natural {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

macro_rules! impl_try_from_ref {
    ($wrapper:ident: $($target:ty),+ $(,)?) => {
        $(
            impl TryFrom<&$wrapper> for $target {
                type Error = ();

                fn try_from(value: &$wrapper) -> Result<Self, Self::Error> {
                    <$target>::try_from(&value.0).map_err(|_| ())
                }
            }
        )+
    };
}

impl_try_from_ref!(Integer: i64, u8, u64, usize);
impl_try_from_ref!(Natural: i64, u64, usize);

macro_rules! impl_binary_ops {
    ($wrapper:ident: $($trait:ident::$method:ident),+ $(,)?) => {
        $(
            impl $trait<&$wrapper> for &$wrapper {
                type Output = $wrapper;

                fn $method(self, rhs: &$wrapper) -> Self::Output {
                    $wrapper((&self.0).$method(&rhs.0))
                }
            }

            impl $trait<$wrapper> for &$wrapper {
                type Output = $wrapper;

                fn $method(self, rhs: $wrapper) -> Self::Output {
                    $wrapper((&self.0).$method(&rhs.0))
                }
            }

            impl $trait<&$wrapper> for $wrapper {
                type Output = $wrapper;

                fn $method(self, rhs: &$wrapper) -> Self::Output {
                    $wrapper(self.0.$method(&rhs.0))
                }
            }

            impl $trait<$wrapper> for $wrapper {
                type Output = $wrapper;

                fn $method(self, rhs: $wrapper) -> Self::Output {
                    $wrapper(self.0.$method(&rhs.0))
                }
            }
        )+
    };
}

impl_binary_ops!(Integer:
    Add::add,
    Sub::sub,
    Mul::mul,
    Div::div,
    Rem::rem,
    BitAnd::bitand,
    BitOr::bitor,
    BitXor::bitxor,
);
impl_binary_ops!(Natural: Add::add, Sub::sub, Mul::mul, Div::div, Rem::rem);

macro_rules! impl_shifts {
    ($wrapper:ident: $($trait:ident::$method:ident),+ $(,)?) => {
        $(
            impl $trait<usize> for $wrapper {
                type Output = $wrapper;

                fn $method(self, rhs: usize) -> Self::Output {
                    $wrapper(self.0.$method(rhs))
                }
            }

            impl $trait<usize> for &$wrapper {
                type Output = $wrapper;

                fn $method(self, rhs: usize) -> Self::Output {
                    $wrapper((&self.0).$method(rhs))
                }
            }
        )+
    };
}

impl_shifts!(Integer: Shl::shl, Shr::shr);
impl_shifts!(Natural: Shl::shl, Shr::shr);

macro_rules! impl_shift_assign {
    ($wrapper:ident: $($trait:ident::$method:ident),+ $(,)?) => {
        $(
            impl $trait<usize> for $wrapper {
                fn $method(&mut self, rhs: usize) {
                    self.0.$method(rhs);
                }
            }
        )+
    };
}

impl_shift_assign!(Integer: ShlAssign::shl_assign, ShrAssign::shr_assign);
impl_shift_assign!(Natural: ShlAssign::shl_assign, ShrAssign::shr_assign);

impl std::ops::BitOrAssign for Integer {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl Neg for Integer {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Integer {
    pub const MAX_BYTE_STRING_OUTPUT_LENGTH: i64 = 8192;
    pub const CARDANO_MAXIMUM_BITS: u64 = 262_143;

    /// Return whether this integer is representable by Cardano's bounded arithmetic builtins.
    pub fn fits_cardano_range(&self) -> bool {
        self.fits_signed_range(Self::CARDANO_MAXIMUM_BITS)
    }

    /// Return whether this integer fits within `[-2^maximum_bits, 2^maximum_bits - 1]`.
    pub fn fits_signed_range(&self, maximum_bits: u64) -> bool {
        let bits = self.bits();

        if bits <= maximum_bits {
            return true;
        }

        if bits != maximum_bits + 1 || !self.is_negative() {
            return false;
        }

        self.unsigned_abs() == Natural::from(1u8) << maximum_bits as usize
    }

    pub fn is_negative(&self) -> bool {
        self < &Self::ZERO
    }

    pub fn is_zero(&self) -> bool {
        self == &Self::ZERO
    }

    pub fn is_greater_than<T>(&self, bound: T) -> bool
    where
        T: Into<Self>,
    {
        self > &bound.into()
    }

    pub fn bits(&self) -> u64 {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            use dashu_base::BitTest;
            self.0.bit_len() as u64
        }

        #[cfg(feature = "malachite")]
        {
            use malachite_base::num::logic::traits::SignificantBits;
            (&self.0).significant_bits()
        }
    }

    pub fn unsigned_abs(&self) -> Natural {
        self.with_unsigned_abs(|value| Natural(value.clone()))
    }

    /// Operate on the absolute value without forcing a clone when the backend can lend it.
    pub fn with_unsigned_abs<R>(&self, f: impl FnOnce(&BackendNatural) -> R) -> R {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            use dashu_base::UnsignedAbs;
            let value = self.0.clone().unsigned_abs();
            f(&value)
        }

        #[cfg(feature = "malachite")]
        {
            f(self.0.unsigned_abs_ref())
        }
    }

    pub fn to_bytes_in<A>(&self, allocator: A, big_endian: bool) -> Option<Vec<u8, A>>
    where
        A: std::alloc::Allocator,
    {
        (!self.is_negative()).then(|| self.to_unsigned_bytes_in(allocator, big_endian))
    }

    fn to_unsigned_bytes_in<A>(&self, allocator: A, big_endian: bool) -> Vec<u8, A>
    where
        A: std::alloc::Allocator,
    {
        debug_assert!(!self.is_negative());

        let byte_len = self.bits().div_ceil(8) as usize;
        let mut bytes = Vec::with_capacity_in(byte_len, allocator);
        bytes.resize(byte_len, 0);

        self.with_unsigned_abs(|value| {
            #[cfg(all(feature = "dashu", not(feature = "malachite")))]
            let iterator = value.as_words();
            #[cfg(feature = "malachite")]
            let iterator = value.as_limbs_asc();

            let mut offset = 0usize;
            for word in iterator {
                let word_bytes = word.to_le_bytes();
                let written = offset.saturating_add(word_bytes.len()).min(byte_len);
                if written > offset {
                    bytes[offset..written].copy_from_slice(&word_bytes[..written - offset]);
                }
                offset = written;
                if offset == byte_len {
                    break;
                }
            }
        });

        if big_endian {
            bytes.reverse();
        }

        bytes
    }

    pub fn div_rem(&self, divisor: &Self) -> (Self, Self) {
        let (quotient, remainder) = {
            #[cfg(all(feature = "dashu", not(feature = "malachite")))]
            use dashu_base::DivRem;
            (&self.0).div_rem(&divisor.0)
        };

        (Self(quotient), Self(remainder))
    }

    pub fn rem_euclid(&self, divisor: &Self) -> Self {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            use dashu_base::RemEuclid;
            Self((&self.0).rem_euclid(&divisor.0).into())
        }

        #[cfg(feature = "malachite")]
        {
            use malachite_base::num::arithmetic::traits::ModEuclidean;

            Self((&self.0).mod_euclidean(&divisor.0).into())
        }
    }

    pub fn rem_euclid_bytes<A>(&self, divisor: &Self, allocator: A) -> Vec<u8, A>
    where
        A: std::alloc::Allocator,
    {
        self.rem_euclid(divisor).to_unsigned_bytes_in(allocator, true)
    }

    /// Divide with a quotient rounded toward negative infinity and a divisor-signed remainder.
    pub fn floor_div_mod(&self, divisor: &Self) -> (Self, Self) {
        let (quotient, remainder) = self.div_rem(divisor);

        if !remainder.is_zero() && self.is_negative() != divisor.is_negative() {
            (quotient - Integer::from(1u8), remainder + divisor)
        } else {
            (quotient, remainder)
        }
    }

    pub fn extended_gcd(self, second: Self) -> (Natural, Self, Self) {
        let mut old_r = self;
        let mut r = second;
        let mut old_s = Self::from(1u8);
        let mut s = Self::from(0u8);
        let mut old_t = Self::from(0u8);
        let mut t = Self::from(1u8);

        while !r.is_zero() {
            let quotient = Self::div_rem(&old_r, &r).0;
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

        (old_r.unsigned_abs(), old_s, old_t)
    }
}

impl Natural {
    /// Raise this base to `exponent` modulo `modulus` without materializing the full power.
    pub fn mod_pow(mut self, mut exponent: Self, modulus: &Self) -> Self {
        let mut result = Self::from(1u8) % modulus;

        while !exponent.is_zero() {
            if exponent.bit(0) {
                result = (result * &self) % modulus;
            }

            exponent >>= 1usize;
            self = (&self * &self) % modulus;
        }

        result
    }

    pub fn is_zero(&self) -> bool {
        self == &Self::ZERO
    }

    pub fn bits(&self) -> u64 {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            use dashu_base::BitTest;
            self.0.bit_len() as u64
        }

        #[cfg(feature = "malachite")]
        {
            use malachite_base::num::logic::traits::SignificantBits;
            self.0.significant_bits()
        }
    }

    pub fn bit(&self, index: u64) -> bool {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            use dashu_base::BitTest;

            self.0.bit(index as usize)
        }

        #[cfg(feature = "malachite")]
        {
            use malachite_base::num::logic::traits::BitAccess;

            self.0.get_bit(index)
        }
    }

    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            Self(BackendNatural::from_be_bytes(bytes))
        }

        #[cfg(feature = "malachite")]
        {
            use malachite_base::num::conversion::traits::PowerOf2Digits;
            Self(
                BackendNatural::from_power_of_2_digits_desc(8, bytes.iter().copied())
                    .unwrap_or_else(|| unreachable!("base-256 bytes are valid 8-bit digits")),
            )
        }
    }

    pub fn from_le_bytes(bytes: &[u8]) -> Self {
        #[cfg(all(feature = "dashu", not(feature = "malachite")))]
        {
            Self(BackendNatural::from_le_bytes(bytes))
        }

        #[cfg(feature = "malachite")]
        {
            use malachite_base::num::conversion::traits::PowerOf2Digits;
            Self(
                BackendNatural::from_power_of_2_digits_asc(8, bytes.iter().copied())
                    .unwrap_or_else(|| unreachable!("base-256 bytes are valid 8-bit digits")),
            )
        }
    }
}

fn decode_integer_with(
    decoder: &mut minicbor::Decoder<'_>,
    check_chunk: impl Fn(&[u8]) -> Result<(), minicbor::decode::Error>,
) -> Result<Integer, minicbor::decode::Error> {
    use minicbor::data::{IanaTag, Type};

    if decoder.datatype()? == Type::Tag {
        let tag = decoder.tag()?;
        return match tag.try_into() {
            Ok(iana @ (IanaTag::PosBignum | IanaTag::NegBignum)) => {
                let mut bytes = Vec::new();
                for chunk in decoder.bytes_iter()? {
                    let chunk = chunk?;
                    check_chunk(chunk)?;
                    bytes.extend_from_slice(chunk);
                }

                let magnitude: Integer = Natural::from_be_bytes(&bytes).into();
                Ok(if iana == IanaTag::PosBignum { magnitude } else { -magnitude - Integer::from(1u8) })
            }
            _ => Err(minicbor::decode::Error::message(format!("unexpected tag for bignum: {tag}"))),
        };
    }

    let i: i128 = decoder.int()?.into();
    Ok(Integer::from(i))
}

pub fn decode_bounded_integer(decoder: &mut minicbor::Decoder<'_>) -> Result<Integer, minicbor::decode::Error> {
    decode_integer_with(decoder, amaru_minicbor_extra::assert_bounded_chunk)
}

impl<'b, C> minicbor::Decode<'b, C> for Integer {
    fn decode(decoder: &mut minicbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        decode_integer_with(decoder, |_| Ok(()))
    }
}

impl<C> minicbor::Encode<C> for Integer {
    fn encode<W: minicbor::encode::Write>(
        &self,
        encoder: &mut minicbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        use amaru_minicbor_extra::encode_bytestring;
        use minicbor::data::Tag;

        let (header, tag, bytes) = if self.is_negative() {
            let minus_self = Integer::ZERO - self;
            if minus_self == Integer::ONE {
                encoder.i8(-1)?;
                return Ok(());
            }
            (
                0x20,
                3,
                (minus_self - Integer::ONE)
                    .to_bytes_in(std::alloc::Global, true)
                    .unwrap_or_else(|| unreachable!("non-negative CBOR bignum magnitude")),
            )
        } else {
            if self.is_zero() {
                encoder.u8(0)?;
                return Ok(());
            }

            (
                0x00,
                2,
                self.to_bytes_in(std::alloc::Global, true)
                    .unwrap_or_else(|| unreachable!("non-negative CBOR bignum magnitude")),
            )
        };

        match bytes.len() {
            1 if bytes[0] <= 0x17 => put(encoder, &[header | bytes[0]])?,
            len @ 1..=8 => {
                let width = len.next_power_of_two();
                put(encoder, &[header | (24 + width.trailing_zeros() as u8)])?;
                let mut buf = [0u8; 8];
                buf[width - len..width].copy_from_slice(&bytes);
                put(encoder, &buf[..width])?;
            }
            _ => {
                encoder.tag(Tag::new(tag))?;
                encode_bytestring(encoder, &bytes)?;
            }
        }

        Ok(())
    }
}

fn put<W: minicbor::encode::Write>(
    encoder: &mut minicbor::Encoder<W>,
    bytes: &[u8],
) -> Result<(), minicbor::encode::Error<W::Error>> {
    encoder.writer_mut().write_all(bytes).map_err(minicbor::encode::Error::write)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{Integer, Natural};
    use crate::arena::Arena;

    #[test]
    fn cbor_integer_cases() {
        for (value, expected) in integer_cases() {
            assert_eq!(encoded_integer(&value), expected, "encoding {value}");

            let bytes = hex::decode(expected).unwrap();
            let decoded: Integer = minicbor::decode(&bytes).unwrap();
            assert_eq!(decoded, value, "roundtrip {value}");
        }
    }

    #[test]
    fn bytes_round_trip_through_heap_and_arena() {
        for value in [
            Integer::from(0u8),
            Integer::from(-1i8),
            Integer::from(255u16),
            Integer::from(256u16),
            Integer::from_str("18446744073709551616").unwrap(),
            Integer::from_str("-18446744073709551617").unwrap(),
        ] {
            for big_endian in [false, true] {
                let magnitude: Integer = value.unsigned_abs().into();
                let expected =
                    magnitude.to_bytes_in(std::alloc::Global, big_endian).expect("integer magnitude is non-negative");
                assert_eq!(
                    if big_endian { Natural::from_be_bytes(&expected) } else { Natural::from_le_bytes(&expected) },
                    magnitude.unsigned_abs()
                );

                let arena = Arena::new();
                assert_eq!(magnitude.to_bytes_in(arena.as_bump(), big_endian).unwrap().as_slice(), expected.as_slice());

                assert_eq!(value.to_bytes_in(std::alloc::Global, big_endian).is_some(), !value.is_negative());
            }
        }
    }

    #[test]
    fn rem_euclid_bytes_are_non_negative() {
        for (value, divisor, expected) in [
            (Integer::from(-1i8), Integer::from(256u16), Integer::from(255u8)),
            (Integer::from(-257i16), Integer::from(256u16), Integer::from(255u8)),
            (Integer::from(256u16), Integer::from(256u16), Integer::ZERO),
        ] {
            let bytes = value.rem_euclid_bytes(&divisor, std::alloc::Global);
            assert_eq!(Integer::from(Natural::from_be_bytes(&bytes)), expected);
        }
    }

    /// Interesting values to exercise the encoder: zero, the small-integer boundary (±23),
    /// the native-integer boundary (±2^64), and arbitrary-precision bignums in both directions.
    /// Each value is paired with its expected canonical CBOR encoding.
    fn integer_cases() -> Vec<(Integer, &'static str)> {
        let two_64 = Integer::ONE << 64usize;
        let big: Integer = Natural::from_be_bytes(&hex::decode("033b2e3c9fd0803ce7ffffff").unwrap()).into();

        vec![
            (Integer::from(0u8), "00"),
            (Integer::from(1u8), "01"),
            (Integer::from(2u8), "02"),
            (Integer::from(-1i8), "20"),
            (Integer::from(-2i8), "21"),
            (Integer::from(23u8), "17"),
            (Integer::from(65536u32), "1a00010000"),
            (Integer::from(-65537i32), "3a00010000"),
            (Integer::from(4294967296u64), "1b0000000100000000"),
            (Integer::from(281474976710656u64), "1b0001000000000000"),
            (&two_64 - &Integer::ONE, "1bffffffffffffffff"),
            (two_64.clone(), "c249010000000000000000"),
            (big.clone(), "c24c033b2e3c9fd0803ce7ffffff"),
            (-big, "c34c033b2e3c9fd0803ce7fffffe"),
            (-two_64, "3bffffffffffffffff"),
            (Integer::from(-23i8), "36"),
        ]
    }

    fn encoded_integer(integer: &Integer) -> String {
        hex::encode(minicbor::to_vec(integer).expect("failed to encode integer"))
    }
}
