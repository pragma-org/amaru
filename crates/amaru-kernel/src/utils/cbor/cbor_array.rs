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

use std::ops::Deref;

use crate::cbor;

/// A struct that maintains a reference to whether a cbor array was indef or not. Useful to
/// specifically test behavior around definite and indefinite CBOR encoded structures.
#[derive(Debug, Clone)]
pub enum CborArray<A> {
    Def(Vec<A>),
    Indef(Vec<A>),
}

impl<A> From<CborArray<A>> for Vec<A> {
    fn from(array: CborArray<A>) -> Self {
        match array {
            CborArray::Def(vec) => vec,
            CborArray::Indef(vec) => vec,
        }
    }
}

impl<A> Deref for CborArray<A> {
    type Target = Vec<A>;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Def(vec) => vec,
            Self::Indef(vec) => vec,
        }
    }
}

impl<C, A> cbor::encode::Encode<C> for CborArray<A>
where
    A: cbor::encode::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::Def(elems) => {
                e.encode_with(elems, ctx)?;
            }
            Self::Indef(elems) => {
                e.begin_array()?;
                for elem in elems.iter() {
                    e.encode_with(elem, ctx)?;
                }
                e.end()?;
            }
        };

        Ok(())
    }
}
