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

use std::collections::BTreeMap;

use amaru_kernel::{BigInt, Constr, Int, MaybeIndefArray, PlutusData};

pub const DEFAULT_TAG: u64 = 102;

pub trait ToConstrTag {
    fn to_constr_tag(&self) -> Option<u64>;
}

impl ToConstrTag for u64 {
    fn to_constr_tag(&self) -> Option<u64> {
        if self <= &6 {
            Some(121 + self)
        } else if self <= &127 {
            Some(1280 - 7 + self)
        } else {
            None
        }
    }
}

pub trait ToPlutusData<const VERSION: u8> {
    fn to_plutus_data(&self) -> PlutusData;
}

pub struct PlutusVersion<const V: u8>;
pub trait IsKnownPlutusVersion {}
impl IsKnownPlutusVersion for PlutusVersion<1> {}
impl IsKnownPlutusVersion for PlutusVersion<2> {}
impl IsKnownPlutusVersion for PlutusVersion<3> {}

impl<const V: u8> ToPlutusData<V> for bool
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        let index = match self {
            false => 0,
            true => 1,
        };
        let constr_tag = index.to_constr_tag();
        PlutusData::Constr(Constr {
            tag: constr_tag.unwrap_or(DEFAULT_TAG),
            any_constructor: constr_tag.map_or(Some(index), |_| None),
            fields: MaybeIndefArray::Indef(vec![]),
        })
    }
}

impl<const V: u8> ToPlutusData<V> for i32
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::BigInt(BigInt::Int(Int::from(*self as i64)))
    }
}

impl<const V: u8> ToPlutusData<V> for i64
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::BigInt(BigInt::Int(Int::from(*self)))
    }
}

impl<const V: u8> ToPlutusData<V> for u32
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::BigInt(BigInt::Int(Int::from(*self as i64)))
    }
}

impl<const V: u8> ToPlutusData<V> for u64
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::BigInt(BigInt::Int(Int::from(*self as i64)))
    }
}

impl<const V: u8> ToPlutusData<V> for usize
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::BigInt(BigInt::Int(Int::from(*self as i64)))
    }
}

impl<const V: u8, T> ToPlutusData<V> for Vec<T>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::Array(MaybeIndefArray::Def(
            self.iter().map(|a| a.to_plutus_data()).collect(),
        ))
    }
}

impl<const V: u8> ToPlutusData<V> for Vec<u8>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::BoundedBytes(self.clone().into())
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for BTreeMap<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Ord,
    V: ToPlutusData<VER>,
{
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::Map(
            self.iter()
                .map(|(k, v)| (k.to_plutus_data(), v.to_plutus_data()))
                .collect(),
        )
    }
}
