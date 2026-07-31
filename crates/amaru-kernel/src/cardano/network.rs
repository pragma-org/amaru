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

use std::fmt;

use crate::cbor;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    std::hash::Hash,
    serde::Serialize,
    serde::Deserialize,
    cbor::Encode,
    cbor::Decode,
)]
#[cbor(index_only)]
pub enum Network {
    #[n(0)]
    Testnet,
    #[n(1)]
    Mainnet,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Testnet => "testnet",
                Self::Mainnet => "mainnet",
            }
        )
    }
}

impl From<Network> for u8 {
    fn from(network: Network) -> u8 {
        match network {
            Network::Testnet => 0,
            Network::Mainnet => 1,
        }
    }
}

impl TryFrom<u8> for Network {
    type Error = ();

    fn try_from(i: u8) -> Result<Self, Self::Error> {
        match i {
            0 => Ok(Self::Testnet),
            1 => Ok(Self::Mainnet),
            _ => Err(()),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::Network;

    pub fn any_network() -> impl Strategy<Value = Network> {
        prop_oneof![Just(Network::Testnet), Just(Network::Mainnet)]
    }
}
