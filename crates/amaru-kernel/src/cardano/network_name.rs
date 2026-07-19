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

use crate::{
    EraHistory, GlobalParameters, MAINNET_DEFAULT_PROTOCOL_PARAMETERS, MAINNET_ERA_HISTORY, MAINNET_GLOBAL_PARAMETERS,
    Network, NetworkMagic, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS,
    PREVIEW_DEFAULT_PROTOCOL_PARAMETERS, PREVIEW_ERA_HISTORY, PREVIEW_GLOBAL_PARAMETERS, ProtocolParameters,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub enum NetworkName {
    Mainnet,
    #[default]
    Preprod,
    Preview,
    Testnet(u32),
}

/// Networks for which Amaru may fetch and embed ledger peer snapshots at build time
/// (for example `mainnet`, `preprod`, and `preview`).
///
/// Custom testnets are not included; additional networks can be added here when ready.
pub const PEER_SNAPSHOT_NETWORKS: &[NetworkName] = &[NetworkName::Mainnet, NetworkName::Preprod, NetworkName::Preview];

impl std::fmt::Display for NetworkName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mainnet => write!(f, "mainnet"),
            Self::Preprod => write!(f, "preprod"),
            Self::Preview => write!(f, "preview"),
            Self::Testnet(magic) => write!(f, "testnet_{}", magic),
        }
    }
}

impl std::str::FromStr for NetworkName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mainnet" => Ok(Self::Mainnet),
            "preprod" => Ok(Self::Preprod),
            "preview" => Ok(Self::Preview),
            _ => {
                let magic = s.strip_prefix("testnet_").ok_or(format!("Invalid network name {}", s))?;
                magic.parse::<u32>().map(NetworkName::Testnet).map_err(|e| e.to_string())
            }
        }
    }
}

impl From<NetworkName> for Network {
    fn from(value: NetworkName) -> Self {
        if value == NetworkName::Mainnet { Network::Mainnet } else { Network::Testnet }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<'de> serde::Deserialize<'de> for NetworkName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use std::str::FromStr;
        let s = String::deserialize(d)?;
        NetworkName::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl NetworkName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Preprod => "preprod",
            Self::Preview => "preview",
            Self::Testnet(_) => "testnet",
        }
    }

    pub fn to_network_magic(self) -> NetworkMagic {
        match self {
            Self::Mainnet => NetworkMagic::MAINNET,
            Self::Preprod => NetworkMagic::PREPROD,
            Self::Preview => NetworkMagic::PREVIEW,
            Self::Testnet(magic) => NetworkMagic::new(magic as u64),
        }
    }

    pub fn as_era_history(&self) -> Option<&EraHistory> {
        match self {
            NetworkName::Mainnet => Some(&MAINNET_ERA_HISTORY),
            NetworkName::Preprod => Some(&PREPROD_ERA_HISTORY),
            NetworkName::Preview => Some(&PREVIEW_ERA_HISTORY),
            NetworkName::Testnet(_) => None,
        }
    }

    pub fn as_global_parameters(&self) -> Option<&'static GlobalParameters> {
        match self {
            NetworkName::Mainnet => Some(&MAINNET_GLOBAL_PARAMETERS),
            NetworkName::Preprod => Some(&PREPROD_GLOBAL_PARAMETERS),
            NetworkName::Preview => Some(&PREVIEW_GLOBAL_PARAMETERS),
            NetworkName::Testnet(_) => None,
        }
    }

    pub fn as_protocol_parameters(&self) -> Option<&ProtocolParameters> {
        match self {
            NetworkName::Mainnet => Some(&MAINNET_DEFAULT_PROTOCOL_PARAMETERS),
            NetworkName::Preprod => Some(&PREPROD_DEFAULT_PROTOCOL_PARAMETERS),
            NetworkName::Preview => Some(&PREVIEW_DEFAULT_PROTOCOL_PARAMETERS),
            NetworkName::Testnet(_) => None,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{prelude::*, prop_oneof};

    use super::NetworkName::{self, *};

    pub fn any_network_name() -> impl Strategy<Value = NetworkName> {
        prop_oneof![Just(Mainnet), Just(Preprod), Just(Preview), (3..u32::MAX).prop_map(Testnet)]
    }
}
