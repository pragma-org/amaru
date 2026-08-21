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

use crate::{cbor, tee, to_cbor};

/// Decode an element and retain its original bytes.
#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Deserialize, serde::Serialize)]
pub struct WithOriginalBytes<A> {
    value: A,
    bytes: Vec<u8>,
}

impl<A> WithOriginalBytes<A> {
    /// Returns `true` if the len is null.
    pub fn new(value: A) -> Self
    where
        A: cbor::encode::Encode<()>,
    {
        Self { bytes: to_cbor(&value), value }
    }

    /// Returns `true` if the len is null.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the original serialised length for this element.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Consume the `WithOriginalBytes` wrapper to get back the element.
    pub fn into_inner(self) -> A {
        self.value
    }
}

impl<A: cbor::encode::Encode<()>> From<A> for WithOriginalBytes<A> {
    fn from(value: A) -> Self {
        Self::new(value)
    }
}

impl<A> Deref for WithOriginalBytes<A> {
    type Target = A;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<'d, A: cbor::decode::Decode<'d, C>, C> cbor::decode::Decode<'d, C> for WithOriginalBytes<A> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (value, bytes) = tee(d, |d| d.decode_with(ctx))?;
        Ok(WithOriginalBytes { bytes: bytes.to_vec(), value })
    }
}

impl<A: cbor::encode::Encode<C>, C> cbor::encode::Encode<C> for WithOriginalBytes<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.bytes).map_err(cbor::encode::Error::write)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::WithOriginalBytes;
    use crate::{from_cbor_no_leftovers, to_cbor};

    #[test]
    fn preserves_original_cbor_encoding() {
        let original = [0x18, 42];
        let value: WithOriginalBytes<u64> = from_cbor_no_leftovers(&original).expect("decode original CBOR");
        assert_eq!(to_cbor(&value), original);
    }
}
