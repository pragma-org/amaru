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

use std::fmt;

use crate::{
    AddressType, Hash, cbor,
    size::{CREDENTIAL, KEY, SCRIPT},
};

// NOTE: Credential variant order
//
// It is tempting to swap the order of the two constructors so that KeyHash
// comes first. This indeed nicely maps the binary representation which
// associates 0 to KeyHash and 1 to ScriptHash.
//
// However, for historical reasons, the ScriptHash variant comes first in the
// Haskell reference codebase. From this ordering is derived the `PartialOrd`
// and `Ord` instances; which impacts how Maps/Dictionnaries indexed by
// Credential will be ordered. So, it is crucial to preserve this quirks to
// avoid hard to troubleshoot issues down the line.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub enum Credential {
    ScriptHash(Hash<{ SCRIPT }>),
    KeyHash(Hash<{ KEY }>),
}

impl Credential {
    pub fn is_script(&self) -> bool {
        matches!(self, Self::ScriptHash(_))
    }

    pub fn from_raw_address(bytes: &[u8]) -> Option<Self> {
        use AddressType::*;
        match AddressType::try_from_header_byte(*bytes.first()?)? {
            Type0 | Type1 => (bytes.len() == 2 * CREDENTIAL + 1).then(|| Self::KeyHash(Hash::from(&bytes[KEY + 1..]))),
            Type2 | Type3 => {
                (bytes.len() == 2 * CREDENTIAL + 1).then(|| Self::ScriptHash(Hash::from(&bytes[SCRIPT + 1..])))
            }
            Type4 | Type5 | Type6 | Type7 | Type8 | Type14 | Type15 => None,
        }
    }
}

impl fmt::Display for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptHash(h) => write!(f, "script({h})"),
            Self::KeyHash(h) => write!(f, "key({h})"),
        }
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Credential {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let variant = d.u16()?;

            match variant {
                0 => Ok(Credential::KeyHash(d.decode_with(ctx)?)),
                1 => Ok(Credential::ScriptHash(d.decode_with(ctx)?)),
                _ => Err(cbor::decode::Error::message("invalid variant id for Credential")),
            }
        })
    }
}

impl<C> cbor::Encode<C> for Credential {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Credential::KeyHash(a) => {
                e.array(2)?;
                e.encode_with(0, ctx)?;
                e.encode_with(a, ctx)?;

                Ok(())
            }
            Credential::ScriptHash(a) => {
                e.array(2)?;
                e.encode_with(1, ctx)?;
                e.encode_with(a, ctx)?;

                Ok(())
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BorrowedCredential<'a> {
    KeyHash(&'a Hash<KEY>),
    ScriptHash(&'a Hash<SCRIPT>),
}

impl<'a> From<&'a Credential> for BorrowedCredential<'a> {
    fn from(value: &'a Credential) -> Self {
        match value {
            Credential::KeyHash(hash) => Self::KeyHash(hash),
            Credential::ScriptHash(hash) => Self::ScriptHash(hash),
        }
    }
}

impl From<BorrowedCredential<'_>> for Credential {
    fn from(value: BorrowedCredential<'_>) -> Self {
        match value {
            BorrowedCredential::KeyHash(hash) => Self::KeyHash(*hash),
            BorrowedCredential::ScriptHash(hash) => Self::ScriptHash(*hash),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{Credential, Hash};

    pub fn any_credential() -> impl Strategy<Value = Credential> {
        prop_oneof![
            any::<[u8; 28]>().prop_map(|hash| Credential::KeyHash(Hash::new(hash))),
            any::<[u8; 28]>().prop_map(|hash| Credential::ScriptHash(Hash::new(hash))),
        ]
    }
}
