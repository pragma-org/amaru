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

use super::{MachineError, env::Env, runtime::Runtime};
use crate::{
    arena::Arena,
    binder::Eval,
    constant::{Constant, Integer, List},
    ledger_value::LedgerValue,
    term::Term,
    typ::Type,
};

#[derive(Debug)]
pub enum Value<'a, V>
where
    V: Eval<'a>,
{
    Con(&'a Constant<'a>),
    Lambda { parameter: &'a V, body: &'a Term<'a, V>, env: &'a Env<'a, V> },
    Builtin(Runtime<'a, V>),
    Delay(&'a Term<'a, V>, &'a Env<'a, V>),
    Constr(usize, &'a [&'a Value<'a, V>]),
}

impl<'a, V> Value<'a, V>
where
    V: Eval<'a>,
{
    pub fn con(arena: &'a Arena, constant: &'a Constant<'a>) -> &'a Value<'a, V> {
        arena.alloc(Value::Con(constant))
    }

    pub fn lambda(arena: &'a Arena, parameter: &'a V, body: &'a Term<'a, V>, env: &'a Env<'a, V>) -> &'a Value<'a, V> {
        arena.alloc(Value::Lambda { parameter, body, env })
    }

    pub fn delay(arena: &'a Arena, body: &'a Term<'a, V>, env: &'a Env<'a, V>) -> &'a Value<'a, V> {
        arena.alloc(Value::Delay(body, env))
    }

    pub fn constr_empty(arena: &'a Arena, tag: usize) -> &'a Value<'a, V> {
        let empty: &'a [&'a Value<'a, V>] = &[];

        arena.alloc(Value::Constr(tag, empty))
    }

    pub fn constr(arena: &'a Arena, tag: usize, values: &'a [&'a Value<'a, V>]) -> &'a Value<'a, V> {
        arena.alloc(Value::Constr(tag, values))
    }

    pub fn builtin(arena: &'a Arena, runtime: Runtime<'a, V>) -> &'a Value<'a, V> {
        arena.alloc(Value::Builtin(runtime))
    }

    pub fn integer(arena: &'a Arena, i: &'a Integer) -> &'a Value<'a, V> {
        let con = arena.alloc(Constant::Integer(i));

        Value::con(arena, con)
    }

    pub fn byte_string(arena: &'a Arena, b: &'a [u8]) -> &'a Value<'a, V> {
        let con = arena.alloc(Constant::ByteString(b));

        Value::con(arena, con)
    }

    pub fn string(arena: &'a Arena, s: &'a str) -> &'a Value<'a, V> {
        let con = arena.alloc(Constant::String(s));

        Value::con(arena, con)
    }

    pub fn bool(arena: &'a Arena, b: bool) -> &'a Value<'a, V> {
        let con = arena.alloc(Constant::Boolean(b));

        Value::con(arena, con)
    }

    pub fn unwrap_integer(&'a self) -> Result<&'a Integer, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Integer(integer) = inner else {
            return Err(MachineError::type_mismatch(Type::Integer, inner));
        };

        Ok(integer)
    }

    pub fn unwrap_byte_string(&'a self) -> Result<&'a [u8], MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::ByteString(byte_string) = inner else {
            return Err(MachineError::type_mismatch(Type::ByteString, inner));
        };

        Ok(byte_string)
    }

    pub fn unwrap_string(&'a self) -> Result<&'a str, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::String(string) = inner else {
            return Err(MachineError::type_mismatch(Type::String, inner));
        };

        Ok(string)
    }

    pub fn unwrap_bool(&'a self) -> Result<bool, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Boolean(b) = inner else {
            return Err(MachineError::type_mismatch(Type::Bool, inner));
        };

        Ok(*b)
    }

    pub fn unwrap_pair(
        &'a self,
    ) -> Result<(&'a Type<'a>, &'a Type<'a>, &'a Constant<'a>, &'a Constant<'a>), MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::ProtoPair(t1, t2, first, second) = inner else {
            return Err(MachineError::expected_pair(inner));
        };

        Ok((t1, t2, first, second))
    }

    pub fn unwrap_list(&'a self) -> Result<(&'a Type<'a>, &'a List<'a>), MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::ProtoList(t1, list) = inner else {
            return Err(MachineError::expected_list(inner));
        };

        Ok((t1, list))
    }

    pub fn unwrap_array(&'a self) -> Result<(&'a Type<'a>, &'a [&'a Constant<'a>]), MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::ProtoArray(t1, array) = inner else {
            return Err(MachineError::expected_array(inner));
        };

        Ok((t1, array))
    }

    pub fn unwrap_map(&'a self) -> Result<(&'a Type<'a>, &'a List<'a>), MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::ProtoList(t1, list) = inner else {
            return Err(MachineError::expected_list(inner));
        };

        Ok((t1, list))
    }

    pub fn unwrap_constant(&'a self) -> Result<&'a Constant<'a>, MachineError<'a, V>> {
        let Value::Con(item) = self else {
            return Err(MachineError::NotAConstant(self));
        };

        Ok(item)
    }

    pub fn unwrap_unit(&'a self) -> Result<(), MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Unit = inner else {
            return Err(MachineError::type_mismatch(Type::Unit, inner));
        };

        Ok(())
    }

    pub(super) fn unwrap_int_list(&'a self) -> Result<&'a List<'a>, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::ProtoList(Type::Integer, list) = inner else {
            return Err(MachineError::type_mismatch(Type::List(&Type::Integer), inner));
        };

        Ok(list)
    }

    pub fn unwrap_bls12_381_g1_element(&'a self) -> Result<&'a blst::blst_p1, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Bls12_381G1Element(g1) = inner else {
            return Err(MachineError::type_mismatch(Type::Bls12_381G1Element, inner));
        };

        Ok(g1)
    }

    pub fn unwrap_bls12_381_g2_element(&'a self) -> Result<&'a blst::blst_p2, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Bls12_381G2Element(g2) = inner else {
            return Err(MachineError::type_mismatch(Type::Bls12_381G2Element, inner));
        };

        Ok(g2)
    }

    pub fn unwrap_bls12_381_ml_result(&'a self) -> Result<&'a blst::blst_fp12, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Bls12_381MlResult(ml_res) = inner else {
            return Err(MachineError::type_mismatch(Type::Bls12_381MlResult, inner));
        };

        Ok(ml_res)
    }

    pub fn unwrap_ledger_value(&'a self) -> Result<&'a LedgerValue<'a>, MachineError<'a, V>> {
        let inner = self.unwrap_constant()?;

        let Constant::Value(v) = inner else {
            return Err(MachineError::type_mismatch(Type::Value, inner));
        };

        Ok(v)
    }
}

impl<'a> Constant<'a> {
    pub fn value<V>(&'a self, arena: &'a Arena) -> &'a Value<'a, V>
    where
        V: Eval<'a>,
    {
        Value::con(arena, self)
    }
}
