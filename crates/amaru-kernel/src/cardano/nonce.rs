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

use std::fmt::Display;

use pallas_crypto::hash::Hasher;

use crate::{Hash, IsHeader, cbor, size::NONCE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct Nonce(Hash<NONCE>);

impl Display for Nonce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Nonce {
    pub fn new(hash: Hash<NONCE>) -> Self {
        Self(hash)
    }

    /// Obtain the final nonce at an epoch boundary for the epoch from the stable candidate and the
    /// last block (header) of the previous epoch.
    ///
    /// Return `None` if header has no parent (i.e. which never happens because all our blocks have
    /// parents in Amaru).
    pub fn make_epoch_nonce<H: IsHeader>(&self, header: &H) -> Option<Nonce> {
        Some(Self::new(Hasher::<256>::hash(&[&self.0[..], &header.parent()?[..]].concat())))
    }

    /// Evolve the current nonce by combining it with the current rolling nonce and the
    /// range-extended tagged leader VRF output.
    ///
    /// Specifically, we combine it with `η` (a.k.a eta), which is a blake2b-256 hash of the
    /// tagged leader VRF output after a range extension. The range extension is, yet another
    /// blake2b-256 hash.
    pub fn evolve<H: IsHeader>(&self, header: &H) -> Nonce {
        Self::new(Hasher::<256>::hash(
            &[&self.0[..], &Hasher::<256>::hash(header.extended_vrf_nonce_output().as_slice())[..]].concat(),
        ))
    }
}

impl From<Hash<NONCE>> for Nonce {
    fn from(hash: Hash<NONCE>) -> Self {
        Self::new(hash)
    }
}

impl From<[u8; NONCE]> for Nonce {
    fn from(bytes: [u8; NONCE]) -> Self {
        Self::new(Hash::from(bytes))
    }
}

impl From<&[u8]> for Nonce {
    fn from(bytes: &[u8]) -> Self {
        Self::new(Hash::from(bytes))
    }
}

impl AsRef<[u8]> for Nonce {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl<C> cbor::encode::Encode<C> for Nonce {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(self.0, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Nonce {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(Self::new(d.decode_with(ctx)?))
    }
}

/// Utility function to parse a nonce (i.e. a blake2b-256 hash digest) from an hex-encoded string.
pub fn parse_nonce(hex_str: &str) -> Result<Nonce, String> {
    hex::decode(hex_str)
        .map_err(|e| format!("invalid hex encoding: {e}"))
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).map_err(|_| "expected 32-byte nonce".to_string()))
        .map(Nonce::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nonce() {
        assert!(matches!(parse_nonce("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d"), Ok(..)));
    }

    #[test]
    fn test_parse_nonce_not_hex() {
        assert!(matches!(parse_nonce("patate"), Err(..)));
    }

    #[test]
    fn test_parse_nonce_too_long() {
        assert!(matches!(parse_nonce("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d1234"), Err(..)));
    }

    #[test]
    fn test_parse_nonce_too_short() {
        assert!(matches!(parse_nonce("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a"), Err(..)));
    }
}
