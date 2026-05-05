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

use crate::cbor;

/// Decode an element and retain its original bytes size.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct WithSize<A> {
    value: A,
    size: usize,
}

impl<A> WithSize<A> {
    /// Instantiate a new sized value from a known size.
    pub fn new(value: A, size: usize) -> Self {
        WithSize { value, size }
    }

    /// Returns `true` if the size is null.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the original serialised length for this element.
    pub fn len(&self) -> usize {
        self.size
    }

    /// Consume the `WithSize` wrapper to get back the element.
    pub fn into_inner(self) -> A {
        self.value
    }
}

impl<A> AsRef<A> for WithSize<A> {
    fn as_ref(&self) -> &A {
        &self.value
    }
}

impl<'d, A: cbor::decode::Decode<'d, C>, C> cbor::decode::Decode<'d, C> for WithSize<A> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let start = d.position();
        let value = d.decode_with(ctx)?;
        let end = d.position();
        Ok(WithSize { size: end - start, value })
    }
}

impl<A: cbor::encode::Encode<C>, C> cbor::encode::Encode<C> for WithSize<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(&self.value, ctx)?;
        Ok(())
    }
}
