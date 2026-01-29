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
    EraHistory, GlobalParameters, MAINNET_ERA_HISTORY, MAINNET_GLOBAL_PARAMETERS, Network,
    NetworkMagic, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, PREVIEW_ERA_HISTORY,
    PREVIEW_GLOBAL_PARAMETERS, Slot, TESTNET_ERA_HISTORY, TESTNET_GLOBAL_PARAMETERS,
};

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum NetworkName {
    Mainnet,
    Preprod,
    Preview,
    Testnet(u32),
}

impl From<NetworkName> for &EraHistory {
    fn from(value: NetworkName) -> Self {
        match value {
            NetworkName::Mainnet => &MAINNET_ERA_HISTORY,
            NetworkName::Preprod => &PREPROD_ERA_HISTORY,
            NetworkName::Preview => &PREVIEW_ERA_HISTORY,
            NetworkName::Testnet(_) => &TESTNET_ERA_HISTORY,
        }
    }
}

impl From<NetworkName> for &GlobalParameters {
    fn from(value: NetworkName) -> Self {
        match value {
            NetworkName::Mainnet => &MAINNET_GLOBAL_PARAMETERS,
            NetworkName::Preprod => &PREPROD_GLOBAL_PARAMETERS,
            NetworkName::Preview => &PREVIEW_GLOBAL_PARAMETERS,
            NetworkName::Testnet(_) => &TESTNET_GLOBAL_PARAMETERS,
        }
    }
}

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
                let magic = s
                    .strip_prefix("testnet_")
                    .ok_or(format!("Invalid network name {}", s))?;

                magic
                    .parse::<u32>()
                    .map(NetworkName::Testnet)
                    .map_err(|e| e.to_string())
            }
        }
    }
}

impl From<NetworkName> for Network {
    fn from(value: NetworkName) -> Self {
        if value == NetworkName::Mainnet {
            Network::Mainnet
        } else {
            Network::Testnet
        }
    }
}

impl NetworkName {
    pub fn to_network_magic(self) -> NetworkMagic {
        match self {
            Self::Mainnet => NetworkMagic::MAINNET,
            Self::Preprod => NetworkMagic::PREPROD,
            Self::Preview => NetworkMagic::PREVIEW,
            Self::Testnet(magic) => NetworkMagic::new(magic as u64),
        }
    }

    /// Compute the default epoch length for this network.
    ///
    /// This is an over-simplification as _theoretically_ each era can
    /// have a different epoch length but in practice, except for
    /// Byron era, all eras for each network have always had the same
    /// length
    pub fn default_epoch_size_in_slots(&self) -> u64 {
        match self {
            NetworkName::Mainnet => 432000,
            NetworkName::Preprod => 432000,
            NetworkName::Preview => 86400,
            NetworkName::Testnet(_) => 86400,
        }
    }

    /// Provide stability window for given network.
    pub fn default_stability_window(&self) -> Slot {
        match self {
            NetworkName::Mainnet => MAINNET_GLOBAL_PARAMETERS.stability_window,
            NetworkName::Preprod => PREPROD_GLOBAL_PARAMETERS.stability_window,
            NetworkName::Preview => PREVIEW_GLOBAL_PARAMETERS.stability_window,
            NetworkName::Testnet(_) => TESTNET_GLOBAL_PARAMETERS.stability_window,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::NetworkName::{self, *};
    use proptest::{prelude::*, prop_oneof};

    pub fn any_network_name() -> impl Strategy<Value = NetworkName> {
        prop_oneof![
            Just(Mainnet),
            Just(Preprod),
            Just(Preview),
            (3..u32::MAX).prop_map(Testnet)
        ]
    }
}
