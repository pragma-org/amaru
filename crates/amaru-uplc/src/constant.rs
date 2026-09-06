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

pub type Integer = num::BigInt;

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
