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

use std::{fmt, ops::Deref};

use crate::{cardano::fixed_bytes::FixedBytes, cbor};

const KEY_SIZE: usize = 32;

/// A VRF verification key, as carried in block headers.
///
/// Distinct from [`crate::VerificationKey`], which is a raw Ed25519 key: both happen to
/// be 32 bytes, but one is never valid where the other is expected.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct VrfVerificationKey(FixedBytes<KEY_SIZE>);

impl VrfVerificationKey {
    /// Size of the key, in bytes.
    pub const SIZE: usize = KEY_SIZE;

    pub const fn zeroes() -> Self {
        Self(FixedBytes::zeroes())
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn as_array(&self) -> &[u8; KEY_SIZE] {
        self.0.as_array()
    }
}

impl fmt::Debug for VrfVerificationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VrfVerificationKey").field(&hex::encode(self.as_slice())).finish()
    }
}

impl fmt::Display for VrfVerificationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Deref for VrfVerificationKey {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl From<[u8; KEY_SIZE]> for VrfVerificationKey {
    fn from(array: [u8; KEY_SIZE]) -> Self {
        Self(FixedBytes::from(array))
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for VrfVerificationKey {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(Self(d.decode_with(ctx)?))
    }
}

impl<C> cbor::Encode<C> for VrfVerificationKey {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

#[cfg(test)]
mod serde_format {
    use super::*;
    use crate::utils::serde::bytes::assert_json_hex_and_cbor_bstr;

    #[test]
    fn json_is_hex_string_and_cbor_is_byte_string() {
        let payload = [0xabu8; KEY_SIZE];
        let value = VrfVerificationKey::from(payload);
        assert_json_hex_and_cbor_bstr(&value, &payload);
    }
}
