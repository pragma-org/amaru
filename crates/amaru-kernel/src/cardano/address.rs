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

use std::{fmt, str::FromStr};

use crate::{AsShelley, HasOwnership, Network, RewardAccount, StakeCredential, cbor, hash, size};

pub mod byron;
pub use byron::ByronAddress;

pub mod shelley;
pub use shelley::ShelleyAddress;

pub mod pointer;
pub use pointer::AddressPointer;

mod delegation_part;
pub use delegation_part::ShelleyDelegationPart;

mod payment_part;
pub use payment_part::ShelleyPaymentPart;

mod address_type;
pub use address_type::AddressType;

#[derive(Debug, Clone, PartialEq, Eq, std::hash::Hash)]
pub enum Address {
    Byron(ByronAddress),
    Shelley(ShelleyAddress),
    // TODO: This is wrong, stake address should be a completely separate type.
    Stake(RewardAccount),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Byron(x) => f.write_str(&x.to_base58()),
            Self::Shelley(x) => f.write_str(&x.to_bech32()),
            Self::Stake(x) => f.write_str(&x.to_bech32()),
        }
    }
}

impl FromStr for Address {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        if let Some(addr) = Address::from_bech32(s) {
            return Ok(addr);
        }

        if let Some(addr) = ByronAddress::from_base58(s) {
            return Ok(Address::Byron(addr));
        }

        if let Some(addr) = Address::from_hex(s) {
            return Ok(addr);
        }

        Err(())
    }
}

impl Address {
    pub fn is_locked_by_script(&self) -> bool {
        matches!(self.as_shelley().map(|addr| addr.owner()), Some(StakeCredential::ScriptHash(_)))
    }

    /// Tries to encode an Address into a bech32 string
    pub fn to_bech32(&self) -> Option<String> {
        match self {
            Address::Byron(_) => None,
            Address::Shelley(x) => Some(x.to_bech32()),
            Address::Stake(x) => Some(x.to_bech32()),
        }
    }

    /// Tries to parse a bech32 value into an Address
    pub fn from_bech32(s: &str) -> Option<Self> {
        let (_hrp, bytes) = bech32::decode(s).ok()?;
        Self::from_bytes(&bytes)
    }

    // Tries to decode the raw bytes of an address
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let header = *bytes.first()?;

        let payload = &bytes[1..];

        match AddressType::try_from_header_byte(header)? {
            AddressType::Type0 => parse_type_0(header, payload),
            AddressType::Type1 => parse_type_1(header, payload),
            AddressType::Type2 => parse_type_2(header, payload),
            AddressType::Type3 => parse_type_3(header, payload),
            AddressType::Type4 => parse_type_4(header, payload),
            AddressType::Type5 => parse_type_5(header, payload),
            AddressType::Type6 => parse_type_6(header, payload),
            AddressType::Type7 => parse_type_7(header, payload),
            AddressType::Type8 => parse_type_8(header, payload),
            AddressType::Type14 => parse_type_14(header, payload),
            AddressType::Type15 => parse_type_15(header, payload),
        }
    }

    // Tries to parse a hex value into an Address
    pub fn from_hex(bytes: &str) -> Option<Self> {
        let bytes = hex::decode(bytes).ok()?;
        Self::from_bytes(&bytes)
    }

    /// Gets the network assoaciated with this address
    pub fn network(&self) -> Network {
        match self {
            Address::Byron(x) => x.network(),
            Address::Shelley(x) => x.network(),
            Address::Stake(x) => x.network(),
        }
    }

    /// Indicates if this is address includes a script hash
    pub fn has_script(&self) -> bool {
        match self {
            Address::Byron(_) => false,
            Address::Shelley(x) => x.has_script(),
            Address::Stake(x) => x.is_script(),
        }
    }

    pub fn to_vec(&self) -> Vec<u8> {
        match self {
            Address::Byron(x) => x.to_vec(),
            Address::Shelley(x) => x.to_vec(),
            Address::Stake(x) => x.to_vec(),
        }
    }

    pub fn to_hex(&self) -> String {
        match self {
            Address::Byron(x) => x.to_hex(),
            Address::Shelley(x) => x.to_hex(),
            Address::Stake(x) => x.to_hex(),
        }
    }
}

fn parse_network(header: u8) -> Option<Network> {
    Network::try_from(header & 0b0000_1111).ok()
}

macro_rules! parse_shelley_fn {
    ($name:tt, $payment:tt, pointer) => {
        fn $name(header: u8, payload: &[u8]) -> Option<Address> {
            if payload.len() < size::CREDENTIAL + 1 {
                return None;
            }

            let net = parse_network(header)?;

            let h1 = hash::try_from_slice::<{ size::CREDENTIAL }>(&payload[0..size::CREDENTIAL])?;
            let p1 = ShelleyPaymentPart::$payment(h1);
            let p2 = ShelleyDelegationPart::try_from_pointer(&payload[size::CREDENTIAL..])?;

            let addr = ShelleyAddress::new(net, p1, p2);

            Some(Address::Shelley(addr))
        }
    };
    ($name:tt, $payment:tt, $delegation:tt) => {
        fn $name(header: u8, payload: &[u8]) -> Option<Address> {
            if payload.len() != 2 * size::CREDENTIAL {
                return None;
            }

            let net = parse_network(header)?;

            let h1 = hash::try_from_slice::<{ size::CREDENTIAL }>(&payload[0..size::CREDENTIAL])?;
            let p1 = ShelleyPaymentPart::$payment(h1);

            let h2 = hash::try_from_slice::<{ size::CREDENTIAL }>(&payload[size::CREDENTIAL..])?;
            let p2 = ShelleyDelegationPart::$delegation(h2);

            let addr = ShelleyAddress::new(net, p1, p2);

            Some(Address::Shelley(addr))
        }
    };
    ($name:tt, $payment:tt) => {
        fn $name(header: u8, payload: &[u8]) -> Option<Address> {
            if payload.len() != size::CREDENTIAL {
                return None;
            }

            let net = parse_network(header)?;
            let h1 = hash::try_from_slice::<{ size::CREDENTIAL }>(&payload[0..size::CREDENTIAL])?;
            let p1 = ShelleyPaymentPart::$payment(h1);

            let addr = ShelleyAddress::new(net, p1, ShelleyDelegationPart::Null);

            Some(Address::Shelley(addr))
        }
    };
}

// types 0-7 are Shelley addresses
parse_shelley_fn!(parse_type_0, from_key_hash, from_key_hash);
parse_shelley_fn!(parse_type_1, from_script_hash, from_key_hash);
parse_shelley_fn!(parse_type_2, from_key_hash, from_script_hash);
parse_shelley_fn!(parse_type_3, from_script_hash, from_script_hash);
parse_shelley_fn!(parse_type_4, from_key_hash, pointer);
parse_shelley_fn!(parse_type_5, from_script_hash, pointer);
parse_shelley_fn!(parse_type_6, from_key_hash);
parse_shelley_fn!(parse_type_7, from_script_hash);

// type 8 (1000) are Byron addresses
fn parse_type_8(header: u8, payload: &[u8]) -> Option<Address> {
    let vec = [&[header], payload].concat();
    let inner = cbor::decode(&vec).ok()?;
    Some(Address::Byron(inner))
}

macro_rules! parse_stake_fn {
    ($name:tt, $type:tt) => {
        fn $name(header: u8, payload: &[u8]) -> Option<Address> {
            if payload.len() != size::CREDENTIAL {
                return None;
            }

            let net = parse_network(header)?;
            let h1 = hash::try_from_slice::<{ size::CREDENTIAL }>(&payload[0..size::CREDENTIAL])?;

            Some(Address::Stake(RewardAccount::new(net, StakeCredential::$type(h1))))
        }
    };
}

// types 14-15 are Stake addresses
parse_stake_fn!(parse_type_14, KeyHash);
parse_stake_fn!(parse_type_15, ScriptHash);

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{Address, Network, ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart, any_hash28};

    pub fn any_shelley_address() -> impl Strategy<Value = Address> {
        (any::<bool>(), any_hash28(), any_hash28()).prop_map(|(is_mainnet, payment_hash, delegation_hash)| {
            let network = if is_mainnet { Network::Mainnet } else { Network::Testnet };

            let payment = ShelleyPaymentPart::Key(payment_hash);
            let delegation = ShelleyDelegationPart::Key(delegation_hash);

            Address::Shelley(ShelleyAddress::new(network, payment, delegation))
        })
    }
}
