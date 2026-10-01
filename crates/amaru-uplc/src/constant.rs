// Copyright 2025 PRAGMA
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

use std::str::FromStr;

use dashu_base::{BitTest, Signed, UnsignedAbs};
use dashu_int::{IBig, UBig};

use crate::{
    arena::Arena, binder::Eval, data::PlutusData, ledger_value::LedgerValue, machine::MachineError, typ::Type,
};

#[derive(Debug, PartialEq)]
pub enum Constant<'a> {
    Integer(&'a Integer),
    ByteString(&'a [u8]),
    String(&'a str),
    Boolean(bool),
    Data(&'a PlutusData<'a>),
    ProtoList(&'a Type<'a>, &'a [&'a Constant<'a>]),
    ProtoArray(&'a Type<'a>, &'a [&'a Constant<'a>]),
    ProtoPair(&'a Type<'a>, &'a Type<'a>, &'a Constant<'a>, &'a Constant<'a>),
    Unit,
    Bls12_381G1Element(&'a blst::blst_p1),
    Bls12_381G2Element(&'a blst::blst_p2),
    Bls12_381MlResult(&'a blst::blst_fp12),
    Value(&'a LedgerValue<'a>),
}

pub type Integer = IBig;
pub(crate) type Natural = UBig;

/// Operations shared by the UPLC runtime and its costing formulas.
///
/// This local trait preserves the ledger's unsigned bit-length interpretation for signed integers.
pub(crate) trait IntegerExt {
    fn bits(&self) -> u64;

    fn is_negative(&self) -> bool;
}

impl IntegerExt for Integer {
    fn bits(&self) -> u64 {
        self.unsigned_abs().bit_len() as u64
    }

    fn is_negative(&self) -> bool {
        Signed::is_negative(self)
    }
}

pub(crate) fn integer_from_bytes(bytes: &[u8], big_endian: bool) -> Integer {
    Integer::from(if big_endian { Natural::from_be_bytes(bytes) } else { Natural::from_le_bytes(bytes) })
}

pub(crate) fn integer_to_bytes(integer: &Integer, big_endian: bool) -> Vec<u8> {
    let magnitude = integer.unsigned_abs();
    if big_endian { magnitude.to_be_bytes().into() } else { magnitude.to_le_bytes().into() }
}

pub(crate) fn integer_from_num_bigint(integer: num_bigint::BigInt) -> Integer {
    Integer::from_str(&integer.to_string()).unwrap_or_else(|_| unreachable!("decimal BigInt must parse as an Integer"))
}

pub(crate) fn integer_to_num_bigint(integer: &Integer) -> num_bigint::BigInt {
    num_bigint::BigInt::from_str(&integer.to_string())
        .unwrap_or_else(|_| unreachable!("decimal Integer must parse as a BigInt"))
}

pub(crate) fn integer_to_usize(integer: &Integer) -> Option<usize> {
    usize::try_from(integer).ok()
}

pub(crate) fn integer_to_u8(integer: &Integer) -> Option<u8> {
    u8::try_from(integer).ok()
}

pub(crate) fn natural_to_u64(natural: &Natural) -> Option<u64> {
    u64::try_from(natural).ok()
}

pub fn integer(arena: &Arena) -> &Integer {
    arena.alloc_integer(Integer::default())
}

pub fn integer_from(arena: &Arena, i: i128) -> &Integer {
    arena.alloc_integer(Integer::from(i))
}

impl<'a> Constant<'a> {
    pub fn integer(arena: &'a Arena, i: &'a Integer) -> &'a Constant<'a> {
        arena.alloc(Constant::Integer(i))
    }

    pub fn integer_from(arena: &'a Arena, i: i128) -> &'a Constant<'a> {
        arena.alloc(Constant::Integer(integer_from(arena, i)))
    }

    pub fn byte_string(arena: &'a Arena, bytes: &'a [u8]) -> &'a Constant<'a> {
        arena.alloc(Constant::ByteString(bytes))
    }

    pub fn string(arena: &'a Arena, s: &'a str) -> &'a Constant<'a> {
        arena.alloc(Constant::String(s))
    }

    pub fn bool(arena: &'a Arena, v: bool) -> &'a Constant<'a> {
        arena.alloc(Constant::Boolean(v))
    }

    pub fn data(arena: &'a Arena, d: &'a PlutusData<'a>) -> &'a Constant<'a> {
        arena.alloc(Constant::Data(d))
    }

    pub fn unit(arena: &'a Arena) -> &'a Constant<'a> {
        arena.alloc(Constant::Unit)
    }

    pub fn proto_list(arena: &'a Arena, inner: &'a Type<'a>, values: &'a [&'a Constant<'a>]) -> &'a Constant<'a> {
        arena.alloc(Constant::ProtoList(inner, values))
    }

    pub fn proto_array(arena: &'a Arena, inner: &'a Type<'a>, values: &'a [&'a Constant<'a>]) -> &'a Constant<'a> {
        arena.alloc(Constant::ProtoArray(inner, values))
    }

    pub fn proto_pair(
        arena: &'a Arena,
        first_type: &'a Type<'a>,
        second_type: &'a Type<'a>,
        first_value: &'a Constant<'a>,
        second_value: &'a Constant<'a>,
    ) -> &'a Constant<'a> {
        arena.alloc(Constant::ProtoPair(first_type, second_type, first_value, second_value))
    }

    pub fn g1(arena: &'a Arena, g1: &'a blst::blst_p1) -> &'a Constant<'a> {
        arena.alloc(Constant::Bls12_381G1Element(g1))
    }

    pub fn g2(arena: &'a Arena, g2: &'a blst::blst_p2) -> &'a Constant<'a> {
        arena.alloc(Constant::Bls12_381G2Element(g2))
    }

    pub fn ml_result(arena: &'a Arena, ml_res: &'a blst::blst_fp12) -> &'a Constant<'a> {
        arena.alloc(Constant::Bls12_381MlResult(ml_res))
    }

    pub fn ledger_value(arena: &'a Arena, v: &'a LedgerValue<'a>) -> &'a Constant<'a> {
        arena.alloc(Constant::Value(v))
    }

    pub fn unwrap_data<V>(&'a self) -> Result<&'a PlutusData<'a>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        #[expect(clippy::wildcard_enum_match_arm)]
        match self {
            Constant::Data(data) => Ok(data),
            _ => Err(MachineError::not_data(self)),
        }
    }

    pub fn type_of(&self, arena: &'a Arena) -> &'a Type<'a> {
        match self {
            Constant::Integer(_) => Type::integer(arena),
            Constant::ByteString(_) => Type::byte_string(arena),
            Constant::String(_) => Type::string(arena),
            Constant::Boolean(_) => Type::bool(arena),
            Constant::Data(_) => Type::data(arena),
            Constant::ProtoList(t, _) => Type::list(arena, t),
            Constant::ProtoArray(t, _) => Type::array(arena, t),
            Constant::ProtoPair(t1, t2, _, _) => Type::pair(arena, t1, t2),
            Constant::Unit => Type::unit(arena),
            Constant::Bls12_381G1Element(_) => Type::g1(arena),
            Constant::Bls12_381G2Element(_) => Type::g2(arena),
            Constant::Bls12_381MlResult(_) => Type::ml_result(arena),
            Constant::Value(_) => Type::value(arena),
        }
    }
}
