// Copyright 2024 PRAGMA
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
use crate::store::minicbor::{decode, encode, Decode, Decoder, Encode, Encoder};
use amaru_kernel::{into_owned_output, TransactionInput};
use iter_borrow::IterBorrow;

pub type Key = TransactionInput;

#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct Value(pub amaru_kernel::TransactionOutput<'static>);

impl<C> Encode<C> for Value {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        e.encode_with(&self.0, ctx)?;
        Ok(())
    }
}

impl<'b, C> Decode<'b, C> for Value {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, decode::Error> {
        let output = d.decode_with(ctx)?;
        Ok(Value(into_owned_output(output)))
    }
}

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Value>>;
