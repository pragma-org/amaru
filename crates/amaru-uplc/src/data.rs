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
    arena::Arena,
    binder::Eval,
    constant::{Constant, Integer, integer_from},
    flat::SimpleCtx,
    machine::MachineError,
};

#[derive(Debug, PartialEq)]
pub enum PlutusData<'a> {
    Constr { tag: u64, fields: &'a [&'a PlutusData<'a>] },
    Map(&'a [(&'a PlutusData<'a>, &'a PlutusData<'a>)]),
    Integer(&'a Integer),
    ByteString(&'a [u8]),
    List(&'a [&'a PlutusData<'a>]),
}

impl<'a> PlutusData<'a> {
    pub fn constr(arena: &'a Arena, tag: u64, fields: &'a [&'a PlutusData<'a>]) -> &'a PlutusData<'a> {
        arena.alloc(PlutusData::Constr { tag, fields })
    }

    pub fn list(arena: &'a Arena, items: &'a [&'a PlutusData<'a>]) -> &'a PlutusData<'a> {
        arena.alloc(PlutusData::List(items))
    }

    pub fn map(arena: &'a Arena, items: &'a [(&'a PlutusData<'a>, &'a PlutusData<'a>)]) -> &'a PlutusData<'a> {
        arena.alloc(PlutusData::Map(items))
    }

    pub fn integer(arena: &'a Arena, i: &'a Integer) -> &'a PlutusData<'a> {
        arena.alloc(PlutusData::Integer(i))
    }

    pub fn integer_from(arena: &'a Arena, i: i128) -> &'a PlutusData<'a> {
        arena.alloc(PlutusData::Integer(integer_from(arena, i)))
    }

    pub fn byte_string(arena: &'a Arena, bytes: &'a [u8]) -> &'a PlutusData<'a> {
        arena.alloc(PlutusData::ByteString(bytes))
    }

    pub fn from_cbor(arena: &'a Arena, cbor: &'_ [u8]) -> Result<&'a PlutusData<'a>, minicbor::decode::Error> {
        minicbor::decode_with(cbor, &mut SimpleCtx { arena })
    }

    pub fn unwrap_constr<V>(&'a self) -> Result<(&'a u64, &'a [&'a PlutusData<'a>]), MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match self {
            PlutusData::Constr { tag, fields } => Ok((tag, fields)),
            _ => Err(MachineError::malformed_data(self)),
        }
    }

    pub fn unwrap_map<V>(&'a self) -> Result<&'a [(&'a PlutusData<'a>, &'a PlutusData<'a>)], MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match self {
            PlutusData::Map(fields) => Ok(fields),
            _ => Err(MachineError::malformed_data(self)),
        }
    }

    pub fn unwrap_integer<V>(&'a self) -> Result<&'a Integer, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match self {
            PlutusData::Integer(i) => Ok(i),
            _ => Err(MachineError::malformed_data(self)),
        }
    }

    pub fn unwrap_byte_string<V>(&'a self) -> Result<&'a [u8], MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match self {
            PlutusData::ByteString(bytes) => Ok(bytes),
            _ => Err(MachineError::malformed_data(self)),
        }
    }

    pub fn unwrap_list<V>(&'a self) -> Result<&'a [&'a PlutusData<'a>], MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match self {
            PlutusData::List(items) => Ok(items),
            _ => Err(MachineError::malformed_data(self)),
        }
    }

    pub fn constant(&'a self, arena: &'a Arena) -> &'a Constant<'a> {
        Constant::data(arena, self)
    }

    pub fn to_bytes<V>(&'a self, arena: &'a Arena) -> Result<&'a [u8], MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        minicbor::to_vec(self)
            .map(|vec| arena.alloc(vec) as &'a [u8])
            .map_err(|_| MachineError::serialization_error(self))
    }
}
