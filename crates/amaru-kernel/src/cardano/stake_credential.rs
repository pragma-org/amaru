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
    Address, HasOwnership, Hash, Network, ShelleyDelegationPart, ShelleyPaymentPart, StakePayload, cbor,
    size::{CREDENTIAL, KEY, SCRIPT},
};

// NOTE: Stake Credential variant order
//
// It is tempting to swap the order of the two constructors so that AddrKeyHash
// comes first. This indeed nicely maps the binary representation which
// associates 0 to AddrKeyHash and 1 to ScriptHash.
//
// However, for historical reasons, the ScriptHash variant comes first in the
// Haskell reference codebase. From this ordering is derived the `PartialOrd`
// and `Ord` instances; which impacts how Maps/Dictionnaries indexed by
// StakeCredential will be ordered. So, it is crucial to preserve this quirks to
// avoid hard to troubleshoot issues down the line.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub enum StakeCredential {
    ScriptHash(Hash<{ SCRIPT }>),
    AddrKeyhash(Hash<{ KEY }>),
}

impl StakeCredential {
    pub fn from_raw_address(bytes: &[u8]) -> Option<Self> {
        match (bytes.first()? & 0b1111_0000) >> 4 {
            0 | 1 if bytes.len() == 2 * CREDENTIAL + 1 => Some(Self::AddrKeyhash(Hash::from(&bytes[KEY + 1..]))),
            2 | 3 if bytes.len() == 2 * CREDENTIAL + 1 => Some(Self::ScriptHash(Hash::from(&bytes[SCRIPT + 1..]))),
            _ => None,
        }
    }
}

impl fmt::Display for StakeCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptHash(h) => write!(f, "script({h})"),
            Self::AddrKeyhash(h) => write!(f, "key({h})"),
        }
    }
}

impl<'b, C> cbor::Decode<'b, C> for StakeCredential {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let variant = d.u16()?;

            match variant {
                0 => Ok(StakeCredential::AddrKeyhash(d.decode_with(ctx)?)),
                1 => Ok(StakeCredential::ScriptHash(d.decode_with(ctx)?)),
                _ => Err(cbor::decode::Error::message("invalid variant id for StakeCredential")),
            }
        })
    }
}

impl<C> cbor::Encode<C> for StakeCredential {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            StakeCredential::AddrKeyhash(a) => {
                e.array(2)?;
                e.encode_with(0, ctx)?;
                e.encode_with(a, ctx)?;

                Ok(())
            }
            StakeCredential::ScriptHash(a) => {
                e.array(2)?;
                e.encode_with(1, ctx)?;
                e.encode_with(a, ctx)?;

                Ok(())
            }
        }
    }
}

// This function shouldn't exist and pallas should provide a RewardAccount = (Network,
// StakeCredential) out of the box instead of row bytes.
pub fn parse_reward_account(bytes: &[u8]) -> Option<(StakeCredential, Network)> {
    if let Some(Address::Stake(address)) = Address::from_bytes(bytes) {
        let network = address.network();
        Some((address.owner(), network))
    } else {
        None
    }
}

impl From<StakePayload> for StakeCredential {
    fn from(payload: StakePayload) -> Self {
        match payload {
            StakePayload::Key(hash) => Self::AddrKeyhash(hash),
            StakePayload::Script(hash) => Self::ScriptHash(hash),
        }
    }
}

impl From<ShelleyPaymentPart> for StakeCredential {
    fn from(part: ShelleyPaymentPart) -> Self {
        match part {
            ShelleyPaymentPart::Key(hash) => Self::AddrKeyhash(hash),
            ShelleyPaymentPart::Script(hash) => Self::ScriptHash(hash),
        }
    }
}

impl TryFrom<ShelleyDelegationPart> for StakeCredential {
    type Error = ();
    fn try_from(part: ShelleyDelegationPart) -> Result<Self, Self::Error> {
        match part {
            ShelleyDelegationPart::Key(hash) => Ok(Self::AddrKeyhash(hash)),
            ShelleyDelegationPart::Script(hash) => Ok(Self::ScriptHash(hash)),
            ShelleyDelegationPart::Pointer(..) | ShelleyDelegationPart::Null => Err(()),
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BorrowedStakeCredential<'a> {
    KeyHash(&'a Hash<KEY>),
    ScriptHash(&'a Hash<SCRIPT>),
}

impl<'a> From<&'a StakeCredential> for BorrowedStakeCredential<'a> {
    fn from(value: &'a StakeCredential) -> Self {
        match value {
            StakeCredential::AddrKeyhash(hash) => Self::KeyHash(hash),
            StakeCredential::ScriptHash(hash) => Self::ScriptHash(hash),
        }
    }
}

impl From<BorrowedStakeCredential<'_>> for StakeCredential {
    fn from(value: BorrowedStakeCredential<'_>) -> Self {
        match value {
            BorrowedStakeCredential::KeyHash(hash) => Self::AddrKeyhash(*hash),
            BorrowedStakeCredential::ScriptHash(hash) => Self::ScriptHash(*hash),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{Hash, StakeCredential};

    pub fn any_stake_credential() -> impl Strategy<Value = StakeCredential> {
        prop_oneof![
            any::<[u8; 28]>().prop_map(|hash| StakeCredential::AddrKeyhash(Hash::new(hash))),
            any::<[u8; 28]>().prop_map(|hash| StakeCredential::ScriptHash(Hash::new(hash))),
        ]
    }
}
