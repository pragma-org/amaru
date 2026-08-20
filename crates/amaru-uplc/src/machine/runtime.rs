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

use core::str;
use std::array::TryFromSliceError;

use bumpalo::collections::{CollectIn, String as BumpString, Vec as BumpVec};
use num::{Integer as NumInteger, Signed, Zero};

use super::{Machine, MachineError, RuntimeError, value::Value};
use crate::{
    arena::Arena,
    binder::Eval,
    bls::{Compressable, SCALAR_PERIOD},
    builtin::DefaultFunction,
    constant::{self, Constant, Integer},
    data::PlutusData,
    ledger_value::{self, LedgerValue, ValueError},
    machine::cost_model::value,
    typ::Type,
};

pub const INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH: i64 = 8192;

/// Check that an integer fits in a signed 4096-bit range: [-(2^4095), 2^4095 - 1].
/// Used by multiScalarMul to limit scalar sizes.
fn check_multi_scalar_range(int: &Integer) -> Result<(), RuntimeError<'_>> {
    let bits = int.bits();

    if bits <= 4095 {
        return Ok(());
    }

    if bits > 4096 {
        return Err(RuntimeError::MultiScalarMulScalarOutOfBounds);
    }

    // bits == 4096: only valid if negative and exactly -(2^4095)
    if !int.is_negative() {
        return Err(RuntimeError::MultiScalarMulScalarOutOfBounds);
    }

    let magnitude = int.magnitude();

    use num::One;

    let two_pow_4095 = num::BigUint::one() << 4095;

    if *magnitude == two_pow_4095 { Ok(()) } else { Err(RuntimeError::MultiScalarMulScalarOutOfBounds) }
}

/// Reduce scalar mod SCALAR_PERIOD, convert to LE bytes, and append to the output buffer.
/// Caller must validate the scalar range first via `check_multi_scalar_range`.
fn prepare_msm_scalar(si: &Integer, scalar_buf: &mut blst::blst_scalar, scalar_bytes: &mut Vec<u8>) {
    let si = si.mod_floor(&SCALAR_PERIOD);
    let (_, be_bytes) = si.to_bytes_be();

    // Zero-padded big-endian scalar on the stack (always 32 bytes).
    const SIZE: usize = size_of::<blst::blst_scalar>();
    let mut padded = [0u8; SIZE];
    padded[SIZE - be_bytes.len()..].copy_from_slice(&be_bytes);

    unsafe {
        blst::blst_scalar_from_bendian(scalar_buf as *mut _, padded.as_ptr() as *const _);
    }
    scalar_bytes.extend_from_slice(&scalar_buf.b);
}

#[derive(Debug)]
pub struct Runtime<'a, V>
where
    V: Eval<'a>,
{
    pub args: BumpVec<'a, &'a Value<'a, V>>,
    pub fun: DefaultFunction,
    pub forces: usize,
}

impl<'a, V> Runtime<'a, V>
where
    V: Eval<'a>,
{
    pub fn new(arena: &'a Arena, fun: DefaultFunction) -> &'a Self {
        arena.alloc(Self { args: BumpVec::new_in(arena.as_bump()), fun, forces: 0 })
    }

    pub fn force(&self, arena: &'a Arena) -> &'a Self {
        arena.alloc(Runtime { args: self.args.clone(), fun: self.fun, forces: self.forces + 1 })
    }

    pub fn push(&self, arena: &'a Arena, arg: &'a Value<'a, V>) -> &'a Self {
        let new_runtime = arena.alloc(Runtime { args: self.args.clone(), fun: self.fun, forces: self.forces });

        new_runtime.args.push(arg);

        new_runtime
    }

    pub fn needs_force(&self) -> bool {
        self.forces < self.fun.force_count()
    }

    pub fn is_arrow(&self) -> bool {
        self.args.len() < self.fun.arity()
    }

    pub fn is_ready(&self) -> bool {
        self.args.len() == self.fun.arity()
    }
}

impl<'a> Machine<'a> {
    pub fn call<V>(&mut self, runtime: &'a Runtime<'a, V>) -> Result<&'a Value<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match runtime.fun {
            DefaultFunction::AddInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::AddInteger, &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::AddInteger))?;

                self.spend_budget(budget)?;

                let result = arg1 + arg2;
                let new = self.arena.alloc_integer(result);

                let value = Value::integer(self.arena, new);

                Ok(value)
            }
            DefaultFunction::AppendByteString => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::AppendByteString,
                        &[value::byte_string_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::AppendByteString))?;

                self.spend_budget(budget)?;

                let mut result = BumpVec::with_capacity_in(arg1.len() + arg2.len(), self.arena.as_bump());

                result.extend_from_slice(arg1);
                result.extend_from_slice(arg2);

                let result = self.arena.alloc(result);

                let value = Value::byte_string(self.arena, result);

                Ok(value)
            }
            DefaultFunction::AppendString => {
                let arg1 = runtime.args[0].unwrap_string()?;
                let arg2 = runtime.args[1].unwrap_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::AppendString, &[value::string_ex_mem(arg1), value::string_ex_mem(arg2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::AppendString))?;

                self.spend_budget(budget)?;

                let mut new = BumpString::new_in(self.arena.as_bump());

                new.push_str(arg1);
                new.push_str(arg2);

                let new = self.arena.alloc(new);

                let value = Value::string(self.arena, new);

                Ok(value)
            }
            DefaultFunction::BData => {
                let b = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::BData, &[value::byte_string_ex_mem(b)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::BData))?;

                self.spend_budget(budget)?;

                let b = PlutusData::byte_string(self.arena, b);

                let value = b.constant(self.arena).value(self.arena);

                Ok(value)
            }
            DefaultFunction::Blake2b_256 => {
                use cryptoxide::{blake2b::Blake2b, digest::Digest};

                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Blake2b_256, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Blake2b_256))?;

                self.spend_budget(budget)?;

                let mut digest = BumpVec::with_capacity_in(32, self.arena.as_bump());

                unsafe {
                    digest.set_len(32);
                }

                let mut context = Blake2b::new(32);

                context.input(arg1);
                context.result(&mut digest);

                let digest = self.arena.alloc(digest);

                let value = Value::byte_string(self.arena, digest);

                Ok(value)
            }
            DefaultFunction::ChooseData => {
                let arg1 = runtime.args[0].unwrap_constant()?.unwrap_data()?;
                let arg2 = runtime.args[1];
                let arg3 = runtime.args[2];
                let arg4 = runtime.args[3];
                let arg5 = runtime.args[4];
                let arg6 = runtime.args[5];

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ChooseData,
                        &[
                            value::data_ex_mem(arg1),
                            value::value_ex_mem(arg2),
                            value::value_ex_mem(arg3),
                            value::value_ex_mem(arg4),
                            value::value_ex_mem(arg5),
                            value::value_ex_mem(arg6),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ChooseData))?;

                self.spend_budget(budget)?;

                match arg1 {
                    PlutusData::Constr { .. } => Ok(arg2),
                    PlutusData::Map(_) => Ok(arg3),
                    PlutusData::List(_) => Ok(arg4),
                    PlutusData::Integer(_) => Ok(arg5),
                    PlutusData::ByteString(_) => Ok(arg6),
                }
            }
            DefaultFunction::ChooseList => {
                let (_, list) = runtime.args[0].unwrap_list()?;
                let arg2 = runtime.args[1];
                let arg3 = runtime.args[2];

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ChooseList,
                        &[value::proto_list_ex_mem(list), value::value_ex_mem(arg2), value::value_ex_mem(arg3)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ChooseList))?;

                self.spend_budget(budget)?;

                if list.is_empty() { Ok(arg2) } else { Ok(arg3) }
            }
            DefaultFunction::ChooseUnit => {
                runtime.args[0].unwrap_unit()?;
                let arg2 = runtime.args[1];

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ChooseUnit, &[value::UNIT_EX_MEM, value::value_ex_mem(arg2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ChooseUnit))?;

                self.spend_budget(budget)?;

                Ok(arg2)
            }
            DefaultFunction::ConsByteString => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ConsByteString,
                        &[value::integer_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ConsByteString))?;

                self.spend_budget(budget)?;

                let byte: u8 = if self.costs.semantics.cons_byte_string_range_checks() {
                    if *arg1 > Integer::from(255) || *arg1 < Integer::from(0) {
                        return Err(MachineError::byte_string_cons_not_a_byte(arg1));
                    }
                    arg1.try_into().expect("should cast to u8 just fine")
                } else {
                    let wrap: Integer = arg1 % 256;
                    wrap.try_into().expect("should cast to u64 just fine")
                };

                let mut ret = BumpVec::with_capacity_in(arg2.len() + 1, self.arena.as_bump());

                ret.push(byte);

                ret.extend_from_slice(arg2);

                let ret = self.arena.alloc(ret);

                let value = Value::byte_string(self.arena, ret);

                Ok(value)
            }
            DefaultFunction::ConstrData => {
                let tag = runtime.args[0].unwrap_integer()?;
                let (typ, fields) = runtime.args[1].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ConstrData,
                        &[value::integer_ex_mem(tag), value::proto_list_ex_mem(fields)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ConstrData))?;

                self.spend_budget(budget)?;

                if *typ != Type::Data {
                    return Err(MachineError::type_mismatch(Type::Data, runtime.args[1].unwrap_constant()?));
                }

                let tag = tag.try_into().expect("should cast to u64 just fine");
                let fields: BumpVec<'_, _> = fields
                    .iter()
                    .map(|d| match d {
                        Constant::Data(d) => *d,
                        _ => unreachable!(),
                    })
                    .collect_in(self.arena.as_bump());
                let fields = self.arena.alloc(fields);

                let data = PlutusData::constr(self.arena, tag, fields);

                let constant = Constant::data(self.arena, data);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::DecodeUtf8 => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::DecodeUtf8, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::DecodeUtf8))?;

                self.spend_budget(budget)?;

                let string = str::from_utf8(arg1).map_err(|e| MachineError::decode_utf8(e))?;

                let value = Value::string(self.arena, string);

                Ok(value)
            }
            DefaultFunction::DivideInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::DivideInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::DivideInteger))?;

                self.spend_budget(budget)?;

                if !arg2.is_zero() {
                    let (result, _) = arg1.div_mod_floor(arg2);

                    let new = self.arena.alloc_integer(result);

                    let value = Value::integer(self.arena, new);

                    Ok(value)
                } else {
                    Err(MachineError::division_by_zero(arg1, arg2))
                }
            }
            DefaultFunction::EncodeUtf8 => {
                let arg1 = runtime.args[0].unwrap_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::EncodeUtf8, &[value::string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::EncodeUtf8))?;

                self.spend_budget(budget)?;

                let s_bytes = arg1.as_bytes();

                let mut bytes = BumpVec::with_capacity_in(s_bytes.len(), self.arena.as_bump());

                bytes.extend_from_slice(s_bytes);

                let bytes = self.arena.alloc(bytes);

                let value = Value::byte_string(self.arena, bytes);

                Ok(value)
            }
            DefaultFunction::EqualsByteString => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::EqualsByteString,
                        &[value::byte_string_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::EqualsByteString))?;

                self.spend_budget(budget)?;

                let result = arg1 == arg2;

                let value = Value::bool(self.arena, result);

                Ok(value)
            }
            DefaultFunction::EqualsData => {
                let d1 = runtime.args[0].unwrap_constant()?.unwrap_data()?;
                let d2 = runtime.args[1].unwrap_constant()?.unwrap_data()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::EqualsData, &[value::data_ex_mem(d1), value::data_ex_mem(d2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::EqualsData))?;

                self.spend_budget(budget)?;

                let value = Value::bool(self.arena, d1.eq(d2));

                Ok(value)
            }
            DefaultFunction::EqualsInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::EqualsInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::EqualsInteger))?;

                self.spend_budget(budget)?;

                let result = arg1 == arg2;

                let value = Value::bool(self.arena, result);

                Ok(value)
            }
            DefaultFunction::EqualsString => {
                let arg1 = runtime.args[0].unwrap_string()?;
                let arg2 = runtime.args[1].unwrap_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::EqualsString, &[value::string_ex_mem(arg1), value::string_ex_mem(arg2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::EqualsString))?;

                self.spend_budget(budget)?;

                let value = Value::bool(self.arena, arg1 == arg2);

                Ok(value)
            }
            DefaultFunction::FstPair => {
                let (_, _, first, second) = runtime.args[0].unwrap_pair()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::FstPair, &[value::pair_ex_mem(first, second)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::FstPair))?;

                self.spend_budget(budget)?;

                let value = Value::con(self.arena, first);

                Ok(value)
            }
            DefaultFunction::HeadList => {
                let (_, list) = runtime.args[0].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::HeadList, &[value::proto_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::HeadList))?;

                self.spend_budget(budget)?;

                if list.is_empty() {
                    Err(MachineError::empty_list(list))
                } else {
                    let value = Value::con(self.arena, list[0]);

                    Ok(value)
                }
            }
            DefaultFunction::IData => {
                let i = runtime.args[0].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::IData, &[value::integer_ex_mem(i)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::IData))?;

                self.spend_budget(budget)?;

                let i = PlutusData::integer(self.arena, i);

                let value = i.constant(self.arena).value(self.arena);

                Ok(value)
            }
            DefaultFunction::IfThenElse => {
                let arg1 = runtime.args[0].unwrap_bool()?;
                let arg2 = runtime.args[1];
                let arg3 = runtime.args[2];
                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::IfThenElse,
                        &[value::BOOL_EX_MEM, value::value_ex_mem(arg2), value::value_ex_mem(arg3)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::IfThenElse))?;
                self.spend_budget(budget)?;

                if arg1 { Ok(arg2) } else { Ok(arg3) }
            }
            DefaultFunction::IndexByteString => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::IndexByteString,
                        &[value::byte_string_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::IndexByteString))?;

                self.spend_budget(budget)?;

                let index: i128 = arg2.try_into().unwrap();

                if 0 <= index && (index as usize) < arg1.len() {
                    let result: Integer = arg1[index as usize].into();
                    let new = self.arena.alloc_integer(result);
                    let value = Value::integer(self.arena, new);

                    Ok(value)
                } else {
                    Err(MachineError::byte_string_out_of_bounds(arg1, arg2))
                }
            }
            DefaultFunction::LengthOfByteString => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::LengthOfByteString, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LengthOfByteString))?;

                self.spend_budget(budget)?;

                let result: Integer = arg1.len().into();

                let new = self.arena.alloc_integer(result);
                let value = Value::integer(self.arena, new);

                Ok(value)
            }
            DefaultFunction::LessThanByteString => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::LessThanByteString,
                        &[value::byte_string_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LessThanByteString))?;

                self.spend_budget(budget)?;

                let result = arg1 < arg2;

                let value = Value::bool(self.arena, result);

                Ok(value)
            }
            DefaultFunction::LessThanEqualsByteString => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::LessThanEqualsByteString,
                        &[value::byte_string_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LessThanEqualsByteString))?;

                self.spend_budget(budget)?;

                let result = arg1 <= arg2;

                let value = Value::bool(self.arena, result);

                Ok(value)
            }
            DefaultFunction::LessThanEqualsInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::LessThanEqualsInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LessThanEqualsInteger))?;

                self.spend_budget(budget)?;

                let result = arg1 <= arg2;

                let value = Value::bool(self.arena, result);

                Ok(value)
            }
            DefaultFunction::LessThanInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::LessThanInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LessThanInteger))?;

                self.spend_budget(budget)?;

                let result = arg1 < arg2;

                let value = Value::bool(self.arena, result);

                Ok(value)
            }
            DefaultFunction::ListData => {
                let (typ, fields) = runtime.args[0].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ListData, &[value::proto_list_ex_mem(fields)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ListData))?;

                self.spend_budget(budget)?;

                if *typ != Type::Data {
                    return Err(MachineError::type_mismatch(Type::Data, runtime.args[0].unwrap_constant()?));
                }

                let fields: BumpVec<'_, _> = fields
                    .iter()
                    .map(|d| match d {
                        Constant::Data(d) => *d,
                        _ => unreachable!(),
                    })
                    .collect_in(self.arena.as_bump());
                let fields = self.arena.alloc(fields);

                let value = PlutusData::list(self.arena, fields).constant(self.arena).value(self.arena);

                Ok(value)
            }
            DefaultFunction::MapData => {
                let (r#type, list) = runtime.args[0].unwrap_list()?;

                if !matches!(r#type, Type::Pair(Type::Data, Type::Data)) {
                    return Err(MachineError::type_mismatch(
                        Type::List(Type::pair(self.arena, Type::data(self.arena), Type::data(self.arena))),
                        runtime.args[0].unwrap_constant()?,
                    ));
                }

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::MapData, &[value::proto_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::MapData))?;

                self.spend_budget(budget)?;

                let mut map = BumpVec::new_in(self.arena.as_bump());

                for item in list {
                    let Constant::ProtoPair(Type::Data, Type::Data, left, right) = item else {
                        unreachable!("is this really unreachable?")
                    };

                    let Constant::Data(key) = left else { unreachable!() };

                    let Constant::Data(value) = right else { unreachable!() };

                    map.push((*key, *value));
                }

                let map = self.arena.alloc(map);

                let value = PlutusData::map(self.arena, map).constant(self.arena).value(self.arena);

                Ok(value)
            }
            DefaultFunction::MkCons => {
                let item = runtime.args[0].unwrap_constant()?;
                let (typ, list) = runtime.args[1].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::MkCons, &[value::constant_ex_mem(item), value::proto_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::MkCons))?;

                self.spend_budget(budget)?;

                if item.type_of(self.arena) != typ {
                    return Err(MachineError::mk_cons_type_mismatch(item));
                }

                let mut new_list = BumpVec::with_capacity_in(list.len() + 1, self.arena.as_bump());

                new_list.push(item);

                new_list.extend_from_slice(list);

                let new_list = self.arena.alloc(new_list);

                let constant = Constant::proto_list(self.arena, typ, new_list);

                let value = constant.value(self.arena);

                Ok(value)
            }
            DefaultFunction::MkNilData => {
                runtime.args[0].unwrap_unit()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::MkNilData, &[value::UNIT_EX_MEM])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::MkNilData))?;

                self.spend_budget(budget)?;

                let list = BumpVec::new_in(self.arena.as_bump());
                let list = self.arena.alloc(list);

                let constant = Constant::proto_list(self.arena, Type::data(self.arena), list);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::MkNilPairData => {
                runtime.args[0].unwrap_unit()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::MkNilPairData, &[value::UNIT_EX_MEM])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::MkNilPairData))?;

                self.spend_budget(budget)?;

                let list = BumpVec::new_in(self.arena.as_bump());
                let list = self.arena.alloc(list);

                let constant = Constant::proto_list(
                    self.arena,
                    Type::pair(self.arena, Type::data(self.arena), Type::data(self.arena)),
                    list,
                );

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::MkPairData => {
                let d1 = runtime.args[0].unwrap_constant()?.unwrap_data()?;
                let d2 = runtime.args[1].unwrap_constant()?.unwrap_data()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::MkPairData, &[value::data_ex_mem(d1), value::data_ex_mem(d2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::MkPairData))?;

                self.spend_budget(budget)?;

                let constant = Constant::proto_pair(
                    self.arena,
                    Type::data(self.arena),
                    Type::data(self.arena),
                    Constant::data(self.arena, d1),
                    Constant::data(self.arena, d2),
                );

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::ModInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ModInteger, &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ModInteger))?;

                self.spend_budget(budget)?;

                if !arg2.is_zero() {
                    let (_, result) = arg1.div_mod_floor(arg2);
                    let result = self.arena.alloc_integer(result);
                    let value = Value::integer(self.arena, result);

                    Ok(value)
                } else {
                    Err(MachineError::division_by_zero(arg1, arg2))
                }
            }
            DefaultFunction::MultiplyInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::MultiplyInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::MultiplyInteger))?;

                self.spend_budget(budget)?;

                let result = arg1 * arg2;

                let new = self.arena.alloc_integer(result);

                let value = Value::integer(self.arena, new);

                Ok(value)
            }
            DefaultFunction::NullList => {
                let (_, list) = runtime.args[0].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::NullList, &[value::proto_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::NullList))?;

                self.spend_budget(budget)?;

                let value = Value::bool(self.arena, list.is_empty());

                Ok(value)
            }
            DefaultFunction::QuotientInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::QuotientInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::QuotientInteger))?;

                self.spend_budget(budget)?;

                if !arg2.is_zero() {
                    let (quotient, _) = arg1.div_rem(arg2);
                    let q = self.arena.alloc_integer(quotient);
                    let value = Value::integer(self.arena, q);
                    Ok(value)
                } else {
                    Err(MachineError::division_by_zero(arg1, arg2))
                }
            }
            DefaultFunction::RemainderInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::RemainderInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::RemainderInteger))?;

                self.spend_budget(budget)?;

                if !arg2.is_zero() {
                    let (_, remainder) = arg1.div_rem(arg2);
                    let r = self.arena.alloc_integer(remainder);
                    let value = Value::integer(self.arena, r);
                    Ok(value)
                } else {
                    Err(MachineError::division_by_zero(arg1, arg2))
                }
            }
            DefaultFunction::SerialiseData => {
                let arg1 = runtime.args[0].unwrap_constant()?.unwrap_data()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::SerialiseData, &[value::data_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::SerialiseData))?;

                self.spend_budget(budget)?;

                let bytes = arg1.to_bytes(self.arena)?;
                let value = Value::byte_string(self.arena, bytes);

                Ok(value)
            }
            DefaultFunction::Sha2_256 => {
                use cryptoxide::{digest::Digest, sha2::Sha256};

                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Sha2_256, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Sha2_256))?;

                self.spend_budget(budget)?;

                let mut hasher = Sha256::new();

                hasher.input(arg1);

                let mut bytes = BumpVec::with_capacity_in(hasher.output_bytes(), self.arena.as_bump());

                unsafe {
                    bytes.set_len(hasher.output_bytes());
                }

                hasher.result(&mut bytes);

                let bytes = self.arena.alloc(bytes);

                let value = Value::byte_string(self.arena, bytes);

                Ok(value)
            }
            DefaultFunction::Sha3_256 => {
                use cryptoxide::{digest::Digest, sha3::Sha3_256};

                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Sha3_256, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Sha3_256))?;

                self.spend_budget(budget)?;

                let mut hasher = Sha3_256::new();

                hasher.input(arg1);

                let mut bytes = BumpVec::with_capacity_in(hasher.output_bytes(), self.arena.as_bump());

                unsafe {
                    bytes.set_len(hasher.output_bytes());
                }

                hasher.result(&mut bytes);

                let bytes = self.arena.alloc(bytes);

                let value = Value::byte_string(self.arena, bytes);

                Ok(value)
            }
            DefaultFunction::SliceByteString => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;
                let arg3 = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::SliceByteString,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2), value::byte_string_ex_mem(arg3)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::SliceByteString))?;

                self.spend_budget(budget)?;

                let skip: usize = if *arg1 < Integer::ZERO {
                    0
                } else if *arg1 > arg3.len().into() {
                    arg3.len()
                } else {
                    arg1.try_into().expect("should cast to usize just fine")
                };

                let take: usize = if *arg2 < Integer::ZERO {
                    0
                } else if *arg2 > arg3.len().into() {
                    arg3.len()
                } else {
                    arg2.try_into().expect("should cast to usize just fine")
                };

                let skip_take: usize = if skip + take > arg3.len() { arg3.len() } else { skip + take };

                let value = Value::byte_string(self.arena, &arg3[skip..(skip_take)]);

                Ok(value)
            }
            DefaultFunction::SndPair => {
                let (_, _, first, second) = runtime.args[0].unwrap_pair()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::SndPair, &[value::pair_ex_mem(first, second)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::SndPair))?;

                self.spend_budget(budget)?;

                let value = Value::con(self.arena, second);

                Ok(value)
            }
            DefaultFunction::SubtractInteger => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::SubtractInteger,
                        &[value::integer_ex_mem(arg1), value::integer_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::SubtractInteger))?;

                self.spend_budget(budget)?;

                let result = arg1 - arg2;

                let new = self.arena.alloc_integer(result);

                let value = Value::integer(self.arena, new);

                Ok(value)
            }
            DefaultFunction::TailList => {
                let (t1, list) = runtime.args[0].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::TailList, &[value::proto_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::TailList))?;

                self.spend_budget(budget)?;

                if list.is_empty() {
                    Err(MachineError::empty_list(list))
                } else {
                    let constant = Constant::proto_list(self.arena, t1, &list[1..]);

                    let value = Value::con(self.arena, constant);

                    Ok(value)
                }
            }
            DefaultFunction::Trace => {
                let arg1 = runtime.args[0].unwrap_string()?;
                let arg2 = runtime.args[1];

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Trace, &[value::string_ex_mem(arg1), value::value_ex_mem(arg2)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Trace))?;

                self.spend_budget(budget)?;

                self.logs.push(arg1.to_string());

                Ok(arg2)
            }
            DefaultFunction::UnBData => {
                let bs = runtime.args[0].unwrap_constant()?.unwrap_data()?.unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnBData, &[value::data_byte_string_ex_mem(bs)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnBData))?;

                self.spend_budget(budget)?;

                let value = Value::byte_string(self.arena, bs);

                Ok(value)
            }
            DefaultFunction::UnConstrData => {
                let (tag, fields) = runtime.args[0].unwrap_constant()?.unwrap_data()?.unwrap_constr()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnConstrData, &[value::data_list_ex_mem(fields)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnConstrData))?;

                self.spend_budget(budget)?;

                let list: BumpVec<'_, _> =
                    fields.iter().map(|d| Constant::data(self.arena, d)).collect_in(self.arena.as_bump());
                let list = self.arena.alloc(list);

                let constant = Constant::proto_pair(
                    self.arena,
                    Type::integer(self.arena),
                    Type::list(self.arena, Type::data(self.arena)),
                    Constant::integer_from(self.arena, *tag as i128),
                    Constant::proto_list(self.arena, Type::data(self.arena), list),
                );

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::UnIData => {
                let i = runtime.args[0].unwrap_constant()?.unwrap_data()?.unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnIData, &[value::data_integer_ex_mem(i)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnIData))?;

                self.spend_budget(budget)?;

                let value = Value::integer(self.arena, i);

                Ok(value)
            }
            DefaultFunction::UnListData => {
                let list = runtime.args[0].unwrap_constant()?.unwrap_data()?.unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnListData, &[value::data_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnListData))?;

                self.spend_budget(budget)?;

                let list: BumpVec<'_, _> =
                    list.iter().map(|d| Constant::data(self.arena, d)).collect_in(self.arena.as_bump());
                let list = self.arena.alloc(list);

                let constant = Constant::proto_list(self.arena, Type::data(self.arena), list);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::UnMapData => {
                let map = runtime.args[0].unwrap_constant()?.unwrap_data()?.unwrap_map()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnMapData, &[value::data_map_ex_mem(map)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnMapData))?;

                self.spend_budget(budget)?;

                let list: BumpVec<'_, _> = map
                    .iter()
                    .map(|(k, v)| {
                        Constant::proto_pair(
                            self.arena,
                            Type::data(self.arena),
                            Type::data(self.arena),
                            Constant::data(self.arena, k),
                            Constant::data(self.arena, v),
                        )
                    })
                    .collect_in(self.arena.as_bump());
                let list = self.arena.alloc(list);

                let constant = Constant::proto_list(
                    self.arena,
                    Type::pair(self.arena, Type::data(self.arena), Type::data(self.arena)),
                    list,
                );

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::VerifyEcdsaSecp256k1Signature => {
                use secp256k1::{Message, PublicKey, Secp256k1, ecdsa::Signature};

                let public_key = runtime.args[0].unwrap_byte_string()?;
                let message = runtime.args[1].unwrap_byte_string()?;
                let signature = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::VerifyEcdsaSecp256k1Signature,
                        &[
                            value::byte_string_ex_mem(public_key),
                            value::byte_string_ex_mem(message),
                            value::byte_string_ex_mem(signature),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::VerifyEcdsaSecp256k1Signature))?;

                self.spend_budget(budget)?;

                let secp = Secp256k1::verification_only();

                let public_key = PublicKey::from_slice(public_key).map_err(MachineError::secp256k1)?;

                let signature = Signature::from_compact(signature).map_err(MachineError::secp256k1)?;

                let message = Message::from_digest_slice(message).map_err(MachineError::secp256k1)?;

                let valid = secp.verify_ecdsa(&message, &signature, &public_key);

                let value = Value::bool(self.arena, valid.is_ok());

                Ok(value)
            }
            DefaultFunction::VerifyEd25519Signature => {
                use cryptoxide::ed25519;

                let public_key = runtime.args[0].unwrap_byte_string()?;
                let message = runtime.args[1].unwrap_byte_string()?;
                let signature = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::VerifyEd25519Signature,
                        &[
                            value::byte_string_ex_mem(public_key),
                            value::byte_string_ex_mem(message),
                            value::byte_string_ex_mem(signature),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::VerifyEd25519Signature))?;

                self.spend_budget(budget)?;

                let public_key: [u8; 32] = public_key
                    .try_into()
                    .map_err(|e: TryFromSliceError| MachineError::unexpected_ed25519_public_key_length(e))?;

                let signature: [u8; 64] = signature
                    .try_into()
                    .map_err(|e: TryFromSliceError| MachineError::unexpected_ed25519_signature_length(e))?;

                let valid = ed25519::verify(message, &public_key, &signature);

                let value = Value::bool(self.arena, valid);

                Ok(value)
            }
            DefaultFunction::VerifySchnorrSecp256k1Signature => {
                use secp256k1::{Secp256k1, XOnlyPublicKey, schnorr::Signature};

                let public_key = runtime.args[0].unwrap_byte_string()?;
                let message = runtime.args[1].unwrap_byte_string()?;
                let signature = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::VerifySchnorrSecp256k1Signature,
                        &[
                            value::byte_string_ex_mem(public_key),
                            value::byte_string_ex_mem(message),
                            value::byte_string_ex_mem(signature),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::VerifySchnorrSecp256k1Signature))?;

                self.spend_budget(budget)?;

                let secp = Secp256k1::verification_only();

                let public_key = XOnlyPublicKey::from_slice(public_key).map_err(MachineError::secp256k1)?;

                let signature = Signature::from_slice(signature).map_err(MachineError::secp256k1)?;

                let valid = secp.verify_schnorr(&signature, message, &public_key);

                let value = Value::bool(self.arena, valid.is_ok());

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_Add => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g1_element()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g1_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G1_Add,
                        &[value::g1_element_ex_mem(), value::g1_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_Add))?;

                self.spend_budget(budget)?;

                let out = self.arena.alloc(blst::blst_p1::default());

                unsafe {
                    blst::blst_p1_add_or_double(out as *mut _, arg1 as *const _, arg2 as *const _);
                }

                let constant = Constant::g1(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_Compress => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g1_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Bls12_381_G1_Compress, &[value::g1_element_ex_mem()])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_Compress))?;

                self.spend_budget(budget)?;

                let out = arg1.compress(self.arena);

                let value = Value::byte_string(self.arena, out);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_Equal => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g1_element()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g1_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G1_Equal,
                        &[value::g1_element_ex_mem(), value::g1_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_Equal))?;

                self.spend_budget(budget)?;

                let is_equal = unsafe { blst::blst_p1_is_equal(arg1, arg2) };

                let value = Value::bool(self.arena, is_equal);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_HashToGroup => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G1_HashToGroup,
                        &[value::byte_string_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_HashToGroup))?;

                self.spend_budget(budget)?;

                if arg2.len() > 255 {
                    return Err(MachineError::hash_to_curve_dst_too_big());
                }

                let out = self.arena.alloc(blst::blst_p1::default());
                let aug = [];

                unsafe {
                    blst::blst_hash_to_g1(
                        out as *mut _,
                        arg1.as_ptr(),
                        arg1.len(),
                        arg2.as_ptr(),
                        arg2.len(),
                        aug.as_ptr(),
                        0,
                    );
                };

                let constant = Constant::g1(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_Neg => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g1_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Bls12_381_G1_Neg, &[value::g1_element_ex_mem()])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_Neg))?;

                self.spend_budget(budget)?;

                let out = self.arena.alloc(*arg1);

                unsafe {
                    // second arg was true in the Cardano code
                    blst::blst_p1_cneg(out as *mut _, true);
                }

                let constant = Constant::g1(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_ScalarMul => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g1_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G1_ScalarMul,
                        &[value::integer_ex_mem(arg1), value::g1_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_ScalarMul))?;

                self.spend_budget(budget)?;

                let size_scalar = size_of::<blst::blst_scalar>();

                let arg1 = arg1.mod_floor(&SCALAR_PERIOD);
                let (_, mut arg1) = arg1.to_bytes_be();

                if size_scalar > arg1.len() {
                    let diff = size_scalar - arg1.len();

                    let mut new_vec = vec![0; diff];

                    new_vec.append(&mut arg1);

                    arg1 = new_vec;
                }

                let out = self.arena.alloc(blst::blst_p1::default());
                let scalar = self.arena.alloc(blst::blst_scalar::default());

                unsafe {
                    blst::blst_scalar_from_bendian(scalar as *mut _, arg1.as_ptr() as *const _);

                    blst::blst_p1_mult(out as *mut _, arg2 as *const _, scalar.b.as_ptr() as *const _, size_scalar * 8);
                }

                let constant = Constant::g1(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G1_Uncompress => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Bls12_381_G1_Uncompress, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_Uncompress))?;

                self.spend_budget(budget)?;

                let out = blst::blst_p1::uncompress(self.arena, arg1).map_err(MachineError::bls)?;

                let constant = Constant::g1(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_Add => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g2_element()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g2_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G2_Add,
                        &[value::g2_element_ex_mem(), value::g2_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_Add))?;

                self.spend_budget(budget)?;

                let out = self.arena.alloc(blst::blst_p2::default());

                unsafe {
                    blst::blst_p2_add_or_double(out as *mut _, arg1 as *const _, arg2 as *const _);
                }

                let constant = Constant::g2(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_Compress => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g2_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Bls12_381_G2_Compress, &[value::g2_element_ex_mem()])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_Compress))?;

                self.spend_budget(budget)?;

                let out = arg1.compress(self.arena);

                let value = Value::byte_string(self.arena, out);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_Equal => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g2_element()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g2_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G2_Equal,
                        &[value::g2_element_ex_mem(), value::g2_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_Equal))?;

                self.spend_budget(budget)?;

                let is_equal = unsafe { blst::blst_p2_is_equal(arg1, arg2) };

                let value = Value::bool(self.arena, is_equal);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_HashToGroup => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;
                let arg2 = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G2_HashToGroup,
                        &[value::byte_string_ex_mem(arg1), value::byte_string_ex_mem(arg2)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_HashToGroup))?;

                self.spend_budget(budget)?;

                if arg2.len() > 255 {
                    return Err(MachineError::hash_to_curve_dst_too_big());
                }

                let out = self.arena.alloc(blst::blst_p2::default());
                let aug = [];

                unsafe {
                    blst::blst_hash_to_g2(
                        out as *mut _,
                        arg1.as_ptr(),
                        arg1.len(),
                        arg2.as_ptr(),
                        arg2.len(),
                        aug.as_ptr(),
                        0,
                    );
                };

                let constant = Constant::g2(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_Neg => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g2_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Bls12_381_G2_Neg, &[value::g2_element_ex_mem()])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_Neg))?;

                self.spend_budget(budget)?;

                let out = self.arena.alloc(*arg1);

                unsafe {
                    // second arg was true in the Cardano code
                    blst::blst_p2_cneg(out as *mut _, true);
                }

                let constant = Constant::g2(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_ScalarMul => {
                let arg1 = runtime.args[0].unwrap_integer()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g2_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G2_ScalarMul,
                        &[value::integer_ex_mem(arg1), value::g2_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_ScalarMul))?;

                self.spend_budget(budget)?;

                let size_scalar = size_of::<blst::blst_scalar>();

                let arg1 = arg1.mod_floor(&SCALAR_PERIOD);

                let (_, mut arg1) = arg1.to_bytes_be();

                if size_scalar > arg1.len() {
                    let diff = size_scalar - arg1.len();

                    let mut new_vec = vec![0; diff];
                    unsafe {
                        new_vec.set_len(diff);
                    }

                    new_vec.append(&mut arg1);

                    arg1 = new_vec;
                }

                let out = self.arena.alloc(blst::blst_p2::default());
                let scalar = self.arena.alloc(blst::blst_scalar::default());

                unsafe {
                    blst::blst_scalar_from_bendian(scalar as *mut _, arg1.as_ptr() as *const _);

                    blst::blst_p2_mult(out as *mut _, arg2 as *const _, scalar.b.as_ptr() as *const _, size_scalar * 8);
                }

                let constant = Constant::g2(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_G2_Uncompress => {
                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Bls12_381_G2_Uncompress, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_Uncompress))?;

                self.spend_budget(budget)?;

                let out = blst::blst_p2::uncompress(self.arena, arg1).map_err(MachineError::bls)?;

                let constant = Constant::g2(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_FinalVerify => {
                let arg1 = runtime.args[0].unwrap_bls12_381_ml_result()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_ml_result()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_FinalVerify,
                        &[value::ml_result_ex_mem(), value::ml_result_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_FinalVerify))?;

                self.spend_budget(budget)?;

                let verified = unsafe { blst::blst_fp12_finalverify(arg1, arg2) };

                let value = Value::bool(self.arena, verified);

                Ok(value)
            }
            DefaultFunction::Bls12_381_MillerLoop => {
                let arg1 = runtime.args[0].unwrap_bls12_381_g1_element()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_g2_element()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_MillerLoop,
                        &[value::g1_element_ex_mem(), value::g2_element_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_MillerLoop))?;

                self.spend_budget(budget)?;

                let out = self.arena.alloc(blst::blst_fp12::default());

                let affine1 = self.arena.alloc(blst::blst_p1_affine::default());
                let affine2 = self.arena.alloc(blst::blst_p2_affine::default());

                unsafe {
                    blst::blst_p1_to_affine(affine1 as *mut _, arg1);
                    blst::blst_p2_to_affine(affine2 as *mut _, arg2);

                    blst::blst_miller_loop(out as *mut _, affine2, affine1);
                }

                let constant = Constant::ml_result(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Bls12_381_MulMlResult => {
                let arg1 = runtime.args[0].unwrap_bls12_381_ml_result()?;
                let arg2 = runtime.args[1].unwrap_bls12_381_ml_result()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_MulMlResult,
                        &[value::ml_result_ex_mem(), value::ml_result_ex_mem()],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_MulMlResult))?;

                self.spend_budget(budget)?;

                let out = self.arena.alloc(blst::blst_fp12::default());

                unsafe {
                    blst::blst_fp12_mul(out as *mut _, arg1, arg2);
                }

                let constant = Constant::ml_result(self.arena, out);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::Keccak_256 => {
                use cryptoxide::{digest::Digest, sha3::Keccak256};

                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Keccak_256, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Keccak_256))?;

                self.spend_budget(budget)?;

                let mut hasher = Keccak256::new();

                hasher.input(arg1);

                let mut bytes = BumpVec::with_capacity_in(hasher.output_bytes(), self.arena.as_bump());

                unsafe {
                    bytes.set_len(hasher.output_bytes());
                }

                hasher.result(&mut bytes);

                let bytes = self.arena.alloc(bytes);

                let value = Value::byte_string(self.arena, bytes);

                Ok(value)
            }
            DefaultFunction::Blake2b_224 => {
                use cryptoxide::{blake2b::Blake2b, digest::Digest};

                let arg1 = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Blake2b_224, &[value::byte_string_ex_mem(arg1)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Blake2b_224))?;

                self.spend_budget(budget)?;

                let mut digest = BumpVec::with_capacity_in(28, self.arena.as_bump());

                unsafe {
                    digest.set_len(28);
                }

                let mut context = Blake2b::new(28);

                context.input(arg1);
                context.result(&mut digest);

                let digest = self.arena.alloc(digest);

                let value = Value::byte_string(self.arena, digest);

                Ok(value)
            }
            DefaultFunction::IntegerToByteString => {
                let endianness = runtime.args[0].unwrap_bool()?;
                let size = runtime.args[1].unwrap_integer()?;
                let input = runtime.args[2].unwrap_integer()?;

                if size.is_negative() {
                    return Err(MachineError::integer_to_byte_string_negative_size(size));
                }

                if *size > INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH.into() {
                    return Err(MachineError::integer_to_byte_string_size_too_big(
                        size,
                        INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH,
                    ));
                }

                let arg1: i64 = i64::try_from(size).unwrap();

                let arg1_exmem = if arg1 == 0 { 0 } else { ((arg1 - 1) / 8) + 1 };

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::IntegerToByteString,
                        &[value::BOOL_EX_MEM, arg1_exmem, value::integer_ex_mem(input)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::IntegerToByteString))?;

                self.spend_budget(budget)?;

                // NOTE:
                // We ought to also check for negative size and too large sizes. These checks
                // however happens prior to calling the builtin as part of the costing step. So by
                // the time we reach this builtin call, the size can be assumed to be
                //
                // >= 0 && < INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH

                if size.is_zero() && value::integer_log2_x(input) >= 8 * INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH {
                    let required = value::integer_log2_x(input) / 8 + 1;

                    return Err(MachineError::integer_to_byte_string_size_too_big(
                        constant::integer_from(self.arena, required as i128),
                        INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH,
                    ));
                }

                if input.is_negative() {
                    return Err(MachineError::integer_to_byte_string_negative_input(input));
                }

                let size_unwrapped: usize = size.try_into().unwrap();

                if input.is_zero() {
                    let mut new_bytes = BumpVec::with_capacity_in(size_unwrapped, self.arena.as_bump());

                    unsafe {
                        new_bytes.set_len(size_unwrapped);
                    }

                    new_bytes.fill(0);

                    let new_bytes = self.arena.alloc(new_bytes);

                    let value = Value::byte_string(self.arena, new_bytes);

                    return Ok(value);
                }

                let mut bytes = if endianness {
                    integer_to_bytes(self.arena, input, true)
                } else {
                    integer_to_bytes(self.arena, input, false)
                };

                if !size.is_zero() && bytes.len() > size_unwrapped {
                    return Err(MachineError::integer_to_byte_string_size_too_small(size, bytes.len()));
                }

                if size_unwrapped > 0 {
                    let padding_size = size_unwrapped - bytes.len();

                    let mut padding = BumpVec::with_capacity_in(padding_size, self.arena.as_bump());

                    unsafe {
                        padding.set_len(padding_size);
                    }

                    padding.fill(0);

                    if endianness {
                        padding.append(&mut bytes);

                        bytes = padding;
                    } else {
                        bytes.append(&mut padding);
                    }
                };

                let bytes = self.arena.alloc(bytes);

                let value = Value::byte_string(self.arena, bytes);

                Ok(value)
            }
            DefaultFunction::ByteStringToInteger => {
                let endianness = runtime.args[0].unwrap_bool()?;
                let bytes = runtime.args[1].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ByteStringToInteger,
                        &[value::BOOL_EX_MEM, value::byte_string_ex_mem(bytes)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ByteStringToInteger))?;

                self.spend_budget(budget)?;

                let number = self.arena.alloc_integer(if endianness {
                    Integer::from_bytes_be(num_bigint::Sign::Plus, bytes)
                } else {
                    Integer::from_bytes_le(num_bigint::Sign::Plus, bytes)
                });

                let value = Value::integer(self.arena, number);

                Ok(value)
            }

            DefaultFunction::AndByteString => {
                let should_pad = runtime.args[0].unwrap_bool()?;
                let left_bytes = runtime.args[1].unwrap_byte_string()?;
                let right_bytes = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::AndByteString,
                        &[
                            value::BOOL_EX_MEM,
                            value::byte_string_ex_mem(left_bytes),
                            value::byte_string_ex_mem(right_bytes),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::AndByteString))?;

                self.spend_budget(budget)?;

                let bytes_result: Vec<u8> = if should_pad {
                    let max_len = left_bytes.len().max(right_bytes.len());
                    (0..max_len)
                        .map(|index| {
                            let left_byte = left_bytes.get(index).copied().unwrap_or(0xFF);
                            let right_byte = right_bytes.get(index).copied().unwrap_or(0xFF);
                            left_byte & right_byte
                        })
                        .collect()
                } else {
                    left_bytes.iter().zip(right_bytes).map(|(b1, b2)| b1 & b2).collect()
                };
                let result = self.arena.alloc(bytes_result);
                let value = Value::byte_string(self.arena, result);
                Ok(value)
            }
            DefaultFunction::OrByteString => {
                let should_pad = runtime.args[0].unwrap_bool()?;
                let left_bytes = runtime.args[1].unwrap_byte_string()?;
                let right_bytes = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::OrByteString,
                        &[
                            value::BOOL_EX_MEM,
                            value::byte_string_ex_mem(left_bytes),
                            value::byte_string_ex_mem(right_bytes),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::OrByteString))?;

                self.spend_budget(budget)?;

                let bytes_result: Vec<u8> = if should_pad {
                    let max_len = left_bytes.len().max(right_bytes.len());
                    (0..max_len)
                        .map(|index| {
                            let left_byte = left_bytes.get(index).copied().unwrap_or(0x00);
                            let right_byte = right_bytes.get(index).copied().unwrap_or(0x00);
                            left_byte | right_byte
                        })
                        .collect()
                } else {
                    left_bytes.iter().zip(right_bytes).map(|(b1, b2)| b1 | b2).collect()
                };

                let result = self.arena.alloc(bytes_result);
                let value = Value::byte_string(self.arena, result);

                Ok(value)
            }
            DefaultFunction::XorByteString => {
                let should_pad = runtime.args[0].unwrap_bool()?;
                let left_bytes = runtime.args[1].unwrap_byte_string()?;
                let right_bytes = runtime.args[2].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::XorByteString,
                        &[
                            value::BOOL_EX_MEM,
                            value::byte_string_ex_mem(left_bytes),
                            value::byte_string_ex_mem(right_bytes),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::XorByteString))?;

                self.spend_budget(budget)?;

                let bytes_result: Vec<u8> = if should_pad {
                    let max_len = left_bytes.len().max(right_bytes.len());
                    (0..max_len)
                        .map(|index| {
                            let left_byte = left_bytes.get(index).copied().unwrap_or(0x00);
                            let right_byte = right_bytes.get(index).copied().unwrap_or(0x00);
                            left_byte ^ right_byte
                        })
                        .collect()
                } else {
                    left_bytes.iter().zip(right_bytes).map(|(b1, b2)| b1 ^ b2).collect()
                };

                let result = self.arena.alloc(bytes_result);
                let value = Value::byte_string(self.arena, result);

                Ok(value)
            }
            DefaultFunction::ComplementByteString => {
                let bytes = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ComplementByteString, &[value::byte_string_ex_mem(bytes)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ComplementByteString))?;
                self.spend_budget(budget)?;

                let result = self.arena.alloc(bytes.iter().map(|b| b ^ 255).collect::<Vec<_>>());

                Ok(Value::byte_string(self.arena, result))
            }
            DefaultFunction::ReadBit => {
                let bytes = runtime.args[0].unwrap_byte_string()?;
                let bit_index = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ReadBit,
                        &[value::byte_string_ex_mem(bytes), value::integer_ex_mem(bit_index)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ReadBit))?;

                self.spend_budget(budget)?;

                if bytes.is_empty() {
                    return Err(MachineError::empty_byte_array());
                }

                if bit_index < &Integer::ZERO || bit_index >= &Integer::from(bytes.len() * 8) {
                    return Err(MachineError::read_bit_out_of_bounds(bit_index, bytes.len() * 8));
                }

                let (byte_index, bit_offset) = bit_index.div_rem(&8.into());
                let bit_offset = usize::try_from(bit_offset).unwrap();

                let flipped_index = bytes.len() - 1 - usize::try_from(byte_index).unwrap();
                let byte = bytes[flipped_index];

                let bit_test = (byte >> bit_offset) & 1 == 1;

                Ok(Value::bool(self.arena, bit_test))
            }
            DefaultFunction::WriteBits => {
                let mut bytes = runtime.args[0].unwrap_byte_string()?.to_vec();
                let indices = runtime.args[1].unwrap_int_list()?;
                let set_bit = runtime.args[2].unwrap_bool()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::WriteBits,
                        &[
                            value::byte_string_ex_mem(bytes.as_slice()),
                            value::proto_list_ex_mem(indices),
                            value::BOOL_EX_MEM,
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::WriteBits))?;

                self.spend_budget(budget)?;

                for index in indices {
                    let Constant::Integer(bit_index) = index else { unreachable!("bit_index must be an integer") };

                    if *bit_index < &Integer::ZERO || *bit_index >= &Integer::from(bytes.len() * 8) {
                        return Err(MachineError::write_bits_out_of_bounds(bit_index, bytes.len() * 8));
                    }

                    let (byte_index, bit_offset) = bit_index.div_rem(&8.into());
                    let bit_offset = usize::try_from(bit_offset).unwrap();
                    let flipped_index = bytes.len() - 1 - usize::try_from(byte_index).unwrap();
                    let bit_mask: u8 = 1 << bit_offset;

                    if set_bit {
                        bytes[flipped_index] |= bit_mask;
                    } else {
                        bytes[flipped_index] &= !bit_mask;
                    }
                }

                let result = self.arena.alloc(bytes);
                Ok(Value::byte_string(self.arena, result))
            }
            DefaultFunction::ReplicateByte => {
                let size = runtime.args[0].unwrap_integer()?;
                let byte = runtime.args[1].unwrap_integer()?;

                if size.is_negative() {
                    return Err(MachineError::replicate_byte_negative_size(size));
                }

                if *size > INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH.into() {
                    return Err(MachineError::replicate_byte_size_too_big(
                        size,
                        INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH,
                    ));
                }

                let arg0: i64 = i64::try_from(size).unwrap();

                let arg0_ex_mem = if arg0 == 0 { 0 } else { ((arg0 - 1) / 8) + 1 };

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ReplicateByte, &[arg0_ex_mem, value::integer_ex_mem(byte)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ReplicateByte))?;

                self.spend_budget(budget)?;

                if size.is_zero() && value::integer_log2_x(byte) >= 8 * INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH {
                    let required = value::integer_log2_x(byte) / 8 + 1;

                    return Err(MachineError::replicate_byte_size_too_big(
                        constant::integer_from(self.arena, required as i128),
                        INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH,
                    ));
                }

                if byte.is_negative() {
                    return Err(MachineError::replicate_byte_negative_input(byte));
                }

                let size: usize = size.try_into().unwrap();

                let Ok(byte) = u8::try_from(byte) else {
                    return Err(MachineError::outside_byte_bounds(byte));
                };

                let result = if size == 0 { self.arena.alloc(vec![]) } else { self.arena.alloc([byte].repeat(size)) };

                Ok(Value::byte_string(self.arena, result))
            }
            DefaultFunction::ShiftByteString => {
                let bytes = runtime.args[0].unwrap_byte_string()?;
                let shift = runtime.args[1].unwrap_integer()?;

                let arg1: i64 =
                    i64::try_from(shift).map_err(|_| MachineError::outside_usize_bounds(shift))?.saturating_abs();

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ShiftByteString, &[value::byte_string_ex_mem(bytes), arg1])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ShiftByteString))?;
                self.spend_budget(budget)?;

                let length = bytes.len();
                let result = self.arena.alloc(vec![0; length]);

                if Integer::from(length) * 8 <= shift.abs() {
                    return Ok(Value::byte_string(self.arena, result));
                }

                let is_shift_left = shift >= &Integer::ZERO;
                let byte_shift = usize::try_from(shift.abs() / 8).unwrap();
                let bit_shift = usize::try_from(shift.abs() % 8).unwrap();

                if is_shift_left {
                    if bit_shift == 0 {
                        // If we can shift entire bytes, that's much simpler
                        let copy_len = length - bit_shift;
                        // For example, consider the following byte array [1,0,1,0,1] being shifted 8 bits (1 byte)
                        // Result: [0,1,0,1,0]
                        result[..copy_len].copy_from_slice(&bytes[byte_shift..]);
                    } else {
                        // This case is a bit trickier, so let's walk through an example:
                        // say we are shifting the following byte string by 12 bits:
                        // [AB CD EF 12]
                        // We know we want to skip the first byte, and shift results 4 bits
                        // In order to shift partial bytes, we need to get the "overflow" from the next byte
                        // That is the complement_shift (in this case 4)
                        // i=0:
                        // src_idx = 0 + 1 = 1
                        // result[0] = CD << 4 = D0
                        // result[0] |= EF >> 4 = D0 | 0E = DE
                        // i=1
                        // src_idx = 1 + 1 = 2
                        // result[1] = EF << 4 = F0
                        // reuslt[1] |= 12 >> 4 = F0 | 01 = F1
                        // i=2
                        // src_idx = 2 + 1 = 3
                        // result[2] = 12 << 4 = 20
                        // 3 + 1  < length = false
                        // So our result is:
                        // [DE F1 20 00]
                        let complement_shift = 8 - bit_shift;
                        #[allow(clippy::needless_range_loop)]
                        for i in 0..(length - byte_shift) {
                            let src_idx = i + byte_shift;

                            result[i] = bytes[src_idx] << bit_shift;
                            if src_idx + 1 < length {
                                result[i] |= bytes[src_idx + 1] >> complement_shift;
                            }
                        }
                    }
                } else {
                    // Right shift has the same logic as left shift with the inverse operations
                    if bit_shift == 0 {
                        let copy_len = length - byte_shift;
                        result[byte_shift..].copy_from_slice(&bytes[..copy_len]);
                    } else {
                        // See left shift case for explanation, but invert all operations
                        let complement_shift = 8 - bit_shift;
                        #[allow(clippy::needless_range_loop)]
                        for i in 0..(length - byte_shift) {
                            let dst_idx = i + byte_shift;
                            result[dst_idx] = bytes[i] >> bit_shift;

                            if i > 0 {
                                result[dst_idx] |= bytes[i - 1] << complement_shift;
                            }
                        }
                    }
                }

                Ok(Value::byte_string(self.arena, result))
            }
            DefaultFunction::RotateByteString => {
                let bytes = runtime.args[0].unwrap_byte_string()?;
                let shift = runtime.args[1].unwrap_integer()?;

                let arg1: i64 =
                    i64::try_from(shift).map_err(|_| MachineError::outside_usize_bounds(shift))?.saturating_abs();

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::RotateByteString, &[value::byte_string_ex_mem(bytes), arg1])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::RotateByteString))?;
                self.spend_budget(budget)?;

                let length = bytes.len();
                let result = self.arena.alloc(bytes.to_vec());

                if bytes.is_empty() {
                    return Ok(Value::byte_string(self.arena, result));
                }

                let shift = shift.mod_floor(&(length * 8).into());
                if shift == Integer::ZERO {
                    return Ok(Value::byte_string(self.arena, result));
                }
                let byte_shift = usize::try_from(&shift / 8).unwrap();
                let bit_shift = usize::try_from(shift % 8).unwrap();

                if bit_shift == 0 {
                    // left rotation is the same as shift left
                    // except the overflowed bits are brought to the right
                    let copy_len = length - byte_shift;

                    result[..copy_len].copy_from_slice(&bytes[byte_shift..(copy_len + byte_shift)]);
                    result[copy_len..].copy_from_slice(&bytes[..byte_shift]);
                } else {
                    let complement_shift = 8 - bit_shift;
                    let wraparound_bits = bytes[0] >> complement_shift;
                    #[allow(clippy::needless_range_loop)]
                    for i in 0..(length - byte_shift) {
                        let src_idx = i + byte_shift;

                        result[i] = bytes[src_idx] << bit_shift;

                        if src_idx + 1 < length {
                            result[i] |= bytes[src_idx + 1] >> complement_shift;
                        } else if byte_shift > 0 {
                            result[i] |= bytes[0] >> complement_shift;
                        } else {
                            // In the case we're doing less than a full byte shift
                            // we still need to wrap the bit
                            result[i] |= wraparound_bits;
                        }
                    }

                    for i in 0..byte_shift {
                        let dst_idx = length - byte_shift + i;
                        result[dst_idx] = bytes[i] << bit_shift;

                        if i + 1 < byte_shift {
                            result[dst_idx] |= bytes[i + 1] >> complement_shift;
                        } else {
                            result[dst_idx] |= bytes[byte_shift] >> complement_shift;
                        }
                    }
                }

                Ok(Value::byte_string(self.arena, result))
            }
            DefaultFunction::CountSetBits => {
                let bytes = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::CountSetBits, &[value::byte_string_ex_mem(bytes)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::CountSetBits))?;
                self.spend_budget(budget)?;

                let weight: Integer = hamming::weight(bytes).into();
                let result = self.arena.alloc_integer(weight);
                Ok(Value::integer(self.arena, result))
            }
            DefaultFunction::FindFirstSetBit => {
                let bytes = runtime.args[0].unwrap_byte_string()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::FindFirstSetBit, &[value::byte_string_ex_mem(bytes)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::FindFirstSetBit))?;
                self.spend_budget(budget)?;

                let first_bit = bytes.iter().rev().enumerate().find_map(|(byte_index, &byte)| {
                    let reversed_byte = byte.reverse_bits();
                    if reversed_byte == 0 {
                        None
                    } else {
                        let bit_index = reversed_byte.leading_zeros() as usize;
                        Some(isize::try_from(bit_index + byte_index * 8).unwrap())
                    }
                });

                let first_bit: Integer = first_bit.unwrap_or(-1).into();
                let result = self.arena.alloc_integer(first_bit);
                Ok(Value::integer(self.arena, result))
            }
            DefaultFunction::Ripemd_160 => {
                use cryptoxide::{digest::Digest, ripemd160::Ripemd160};
                let input = runtime.args[0].unwrap_byte_string()?;
                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::Ripemd_160, &[value::byte_string_ex_mem(input)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Ripemd_160))?;
                self.spend_budget(budget)?;

                let mut hasher = Ripemd160::new();
                hasher.input(input);
                let result = self.arena.alloc(vec![0; hasher.output_bytes()]);
                hasher.result(result);

                Ok(Value::byte_string(self.arena, result))
            }
            DefaultFunction::ExpModInteger => {
                let base = runtime.args[0].unwrap_integer()?;
                let exponent = runtime.args[1].unwrap_integer()?;
                let modulus = runtime.args[2].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ExpModInteger,
                        &[value::integer_ex_mem(base), value::integer_ex_mem(exponent), value::integer_ex_mem(modulus)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ExpModInteger))?;
                self.spend_budget(budget)?;

                if modulus <= &Integer::ZERO {
                    return Err(MachineError::division_by_zero(base, modulus));
                }

                let result = if exponent.is_negative() {
                    match base.modinv(modulus) {
                        Some(inv) => inv.modpow(&exponent.abs(), modulus),
                        None => return Err(MachineError::ExplicitErrorTerm),
                    }
                } else {
                    base.modpow(exponent, modulus)
                };

                let value = Value::integer(self.arena, self.arena.alloc_integer(result));
                Ok(value)
            }
            DefaultFunction::DropList => {
                let elements_to_drop = runtime.args[0].unwrap_integer()?;
                let (list_type, list) = runtime.args[1].unwrap_list()?;

                let arg0: i64 = u64::try_from(elements_to_drop.abs()).unwrap().try_into().unwrap_or(i64::MAX);

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::DropList, &[arg0, value::proto_list_ex_mem(list)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::DropList))?;

                self.spend_budget(budget)?;

                if elements_to_drop.is_negative() {
                    let constant = Constant::proto_list(self.arena, list_type, list);
                    let value = Value::con(self.arena, constant);
                    return Ok(value);
                }

                let elements_to_drop_usize = if *elements_to_drop > (usize::MAX as i128).into() {
                    list.len()
                } else {
                    usize::try_from(elements_to_drop).unwrap_or(0)
                };

                let remaining_list =
                    if elements_to_drop_usize >= list.len() { &[] } else { &list[elements_to_drop_usize..] };

                let constant = Constant::proto_list(self.arena, list_type, remaining_list);
                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::LengthOfArray => {
                let (_, array) = runtime.args[0].unwrap_array()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::LengthOfArray, &[value::proto_list_ex_mem(array)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LengthOfArray))?;

                self.spend_budget(budget)?;

                let result: Integer = array.len().into();
                let new = self.arena.alloc_integer(result);
                let value = Value::integer(self.arena, new);

                Ok(value)
            }
            DefaultFunction::ListToArray => {
                let (list_type, list) = runtime.args[0].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::ListToArray,
                        &[value::proto_list_ex_mem(list), value::proto_list_ex_mem(list)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ListToArray))?;

                self.spend_budget(budget)?;

                let constant = Constant::proto_array(self.arena, list_type, list);

                let value = Value::con(self.arena, constant);

                Ok(value)
            }
            DefaultFunction::IndexArray => {
                let (_, array) = runtime.args[0].unwrap_array()?;
                let arg1 = runtime.args[1].unwrap_integer()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::IndexArray,
                        &[value::proto_list_ex_mem(array), value::integer_ex_mem(arg1)],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::IndexArray))?;
                self.spend_budget(budget)?;

                let index: i128 = arg1.try_into().unwrap();

                if 0 <= index && (index as usize) < array.len() {
                    let element = array[index as usize];
                    let value = Value::con(self.arena, element);
                    Ok(value)
                } else {
                    Err(MachineError::index_array_out_of_bounds(arg1, array.len()))
                }
            }
            DefaultFunction::Bls12_381_G1_MultiScalarMul => {
                let (_, scalars) = runtime.args[0].unwrap_list()?;
                let (_, points) = runtime.args[1].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G1_MultiScalarMul,
                        &[scalars.len() as i64, points.len() as i64],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G1_MultiScalarMul))?;

                self.spend_budget(budget)?;

                let n = scalars.len().min(points.len());
                let size_scalar = size_of::<blst::blst_scalar>();

                let mut scalar_bytes = Vec::with_capacity(n * size_scalar);
                let mut proj_points = Vec::with_capacity(n);
                let mut scalar_buf = blst::blst_scalar::default();

                for i in 0..n {
                    let Constant::Integer(si) = scalars[i] else {
                        return Err(MachineError::type_mismatch(Type::Integer, scalars[i]));
                    };

                    let Constant::Bls12_381G1Element(pi) = points[i] else {
                        return Err(MachineError::type_mismatch(Type::Bls12_381G1Element, points[i]));
                    };

                    // Validate range even for infinity points.
                    check_multi_scalar_range(si).map_err(MachineError::runtime)?;

                    // Skip infinity points: scalar * infinity = identity,
                    // and infinity poisons the batch affine conversion.
                    if unsafe { blst::blst_p1_is_inf(*pi) } {
                        continue;
                    }

                    prepare_msm_scalar(si, &mut scalar_buf, &mut scalar_bytes);

                    proj_points.push(**pi);
                }

                let result = if proj_points.is_empty() {
                    let compressed: [u8; 48] = {
                        let mut buf = [0u8; 48];
                        buf[0] = 0xc0;
                        buf
                    };

                    *blst::blst_p1::uncompress(self.arena, &compressed).map_err(MachineError::bls)?
                } else {
                    let affines = blst::p1_affines::from(&proj_points);
                    affines.mult(&scalar_bytes, size_scalar * 8)
                };

                let out = self.arena.alloc(result);

                let constant = Constant::g1(self.arena, out);

                Ok(Value::con(self.arena, constant))
            }
            DefaultFunction::Bls12_381_G2_MultiScalarMul => {
                let (_, scalars) = runtime.args[0].unwrap_list()?;
                let (_, points) = runtime.args[1].unwrap_list()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::Bls12_381_G2_MultiScalarMul,
                        &[scalars.len() as i64, points.len() as i64],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::Bls12_381_G2_MultiScalarMul))?;

                self.spend_budget(budget)?;

                let n = scalars.len().min(points.len());
                let size_scalar = size_of::<blst::blst_scalar>();

                let mut scalar_bytes = Vec::with_capacity(n * size_scalar);
                let mut proj_points = Vec::with_capacity(n);
                let mut scalar_buf = blst::blst_scalar::default();

                for i in 0..n {
                    let Constant::Integer(si) = scalars[i] else {
                        return Err(MachineError::type_mismatch(Type::Integer, scalars[i]));
                    };

                    let Constant::Bls12_381G2Element(pi) = points[i] else {
                        return Err(MachineError::type_mismatch(Type::Bls12_381G2Element, points[i]));
                    };

                    // Validate range even for infinity points.
                    check_multi_scalar_range(si).map_err(MachineError::runtime)?;

                    // Skip infinity points: scalar * infinity = identity,
                    // and infinity poisons the batch affine conversion.
                    if unsafe { blst::blst_p2_is_inf(*pi) } {
                        continue;
                    }

                    prepare_msm_scalar(si, &mut scalar_buf, &mut scalar_bytes);

                    proj_points.push(**pi);
                }

                let result = if proj_points.is_empty() {
                    let compressed: [u8; 96] = {
                        let mut buf = [0u8; 96];
                        buf[0] = 0xc0;
                        buf
                    };

                    *blst::blst_p2::uncompress(self.arena, &compressed).map_err(MachineError::bls)?
                } else {
                    let affines = blst::p2_affines::from(&proj_points);
                    affines.mult(&scalar_bytes, size_scalar * 8)
                };

                let out = self.arena.alloc(result);

                let constant = Constant::g2(self.arena, out);

                Ok(Value::con(self.arena, constant))
            }

            DefaultFunction::InsertCoin => {
                let ccy = runtime.args[0].unwrap_byte_string()?;
                let tok = runtime.args[1].unwrap_byte_string()?;
                let qty = runtime.args[2].unwrap_integer()?;
                let v = runtime.args[3].unwrap_ledger_value()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::InsertCoin,
                        &[
                            value::byte_string_ex_mem(ccy),
                            value::byte_string_ex_mem(tok),
                            value::integer_ex_mem(qty),
                            ledger_value::value_max_depth(v),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::InsertCoin))?;

                self.spend_budget(budget)?;

                // Validate quantity in 128-bit signed range
                if !qty.is_zero() {
                    ledger_value::check_quantity_range(qty).map_err(|e| MachineError::runtime(e.into()))?;
                }

                // Validate key lengths (> 32 only allowed when qty=0, which is a no-op)
                if ccy.len() > 32 || tok.len() > 32 {
                    if qty.is_zero() {
                        let constant = Constant::ledger_value(self.arena, v);
                        return Ok(Value::con(self.arena, constant));
                    }

                    let err = if ccy.len() > 32 {
                        ValueError::InsertCoinInvalidCurrency
                    } else {
                        ValueError::InsertCoinInvalidToken
                    };

                    return Err(MachineError::runtime(err.into()));
                }

                let result = LedgerValue::insert_coin(self.arena, ccy, tok, qty, v);

                let constant = Constant::ledger_value(self.arena, result);

                Ok(Value::con(self.arena, constant))
            }
            DefaultFunction::LookupCoin => {
                let ccy = runtime.args[0].unwrap_byte_string()?;
                let tok = runtime.args[1].unwrap_byte_string()?;
                let v = runtime.args[2].unwrap_ledger_value()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(
                        DefaultFunction::LookupCoin,
                        &[
                            value::byte_string_ex_mem(ccy),
                            value::byte_string_ex_mem(tok),
                            ledger_value::value_max_depth(v),
                        ],
                    )
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::LookupCoin))?;

                self.spend_budget(budget)?;

                let qty = v.lookup_coin(self.arena, ccy, tok);

                Ok(Value::integer(self.arena, qty))
            }
            DefaultFunction::UnionValue => {
                let v1 = runtime.args[0].unwrap_ledger_value()?;
                let v2 = runtime.args[1].unwrap_ledger_value()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnionValue, &[v1.size as i64, v2.size as i64])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnionValue))?;

                self.spend_budget(budget)?;

                let result =
                    LedgerValue::union_value(self.arena, v1, v2).map_err(|e| MachineError::runtime(e.into()))?;

                let constant = Constant::ledger_value(self.arena, result);

                Ok(Value::con(self.arena, constant))
            }
            DefaultFunction::ValueContains => {
                let v1 = runtime.args[0].unwrap_ledger_value()?;
                let v2 = runtime.args[1].unwrap_ledger_value()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ValueContains, &[v1.size as i64, v2.size as i64])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ValueContains))?;

                self.spend_budget(budget)?;

                let result = LedgerValue::value_contains(v1, v2).map_err(|e| MachineError::runtime(e.into()))?;

                Ok(Value::bool(self.arena, result))
            }
            DefaultFunction::ValueData => {
                let v = runtime.args[0].unwrap_ledger_value()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ValueData, &[v.size as i64])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ValueData))?;

                self.spend_budget(budget)?;

                let data = LedgerValue::value_data(self.arena, v).map_err(|e| MachineError::runtime(e.into()))?;

                let constant = Constant::data(self.arena, data);

                Ok(Value::con(self.arena, constant))
            }
            DefaultFunction::UnValueData => {
                let data = runtime.args[0].unwrap_constant()?.unwrap_data()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::UnValueData, &[ledger_value::data_node_count(data)])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::UnValueData))?;

                self.spend_budget(budget)?;

                let result =
                    LedgerValue::un_value_data(self.arena, data).map_err(|e| MachineError::runtime(e.into()))?;

                let constant = Constant::ledger_value(self.arena, result);

                Ok(Value::con(self.arena, constant))
            }
            DefaultFunction::ScaleValue => {
                let scalar = runtime.args[0].unwrap_integer()?;
                let v = runtime.args[1].unwrap_ledger_value()?;

                let budget = self
                    .costs
                    .builtin_costs
                    .get_cost(DefaultFunction::ScaleValue, &[value::integer_ex_mem(scalar), v.size as i64])
                    .ok_or(MachineError::NoCostForBuiltin(DefaultFunction::ScaleValue))?;

                self.spend_budget(budget)?;

                let result =
                    LedgerValue::scale_value(self.arena, scalar, v).map_err(|e| MachineError::runtime(e.into()))?;

                let constant = Constant::ledger_value(self.arena, result);

                Ok(Value::con(self.arena, constant))
            }
        }
    }
}

fn integer_to_bytes<'a>(arena: &'a Arena, num: &'a Integer, big_endian: bool) -> BumpVec<'a, u8> {
    let bytes = if big_endian { num.magnitude().to_bytes_be() } else { num.magnitude().to_bytes_le() };

    let mut result = BumpVec::with_capacity_in(bytes.len(), arena.as_bump());
    result.extend_from_slice(&bytes);
    result
}
