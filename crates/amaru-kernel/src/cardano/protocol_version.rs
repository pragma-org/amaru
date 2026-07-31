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

use crate::HasMajorVersion;

pub type ProtocolVersion = (u64, u64);

pub const PROTOCOL_VERSION_10: ProtocolVersion = (10, 0);

pub const PROTOCOL_VERSION_11: ProtocolVersion = (11, 0);

pub const DEFAULT: ProtocolVersion = PROTOCOL_VERSION_11;

pub const MINIMUM_SUPPORTED: ProtocolVersion = PROTOCOL_VERSION_10;

impl HasMajorVersion for ProtocolVersion {
    fn major(&self) -> u32 {
        self.0 as u32
    }
}

pub fn fmt(version: &ProtocolVersion) -> String {
    format!("{}.{}", version.0, version.1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("protocol version {}.{} is too old; minimum supported version is {}.{}", snapshot_version.0, snapshot_version.1, minimum_version.0, minimum_version.1)]
pub struct ProtocolVersionTooOld {
    pub snapshot_version: ProtocolVersion,
    pub minimum_version: ProtocolVersion,
}

pub fn validate(version: ProtocolVersion, minimum: ProtocolVersion) -> Result<(), ProtocolVersionTooOld> {
    if version < minimum {
        return Err(ProtocolVersionTooOld { snapshot_version: version, minimum_version: minimum });
    }
    Ok(())
}

#[cfg(any(test, feature = "test-utils"))]
pub use proxy::*;

#[cfg(any(test, feature = "test-utils"))]
mod proxy {
    use serde::Deserialize;

    use super::ProtocolVersion;
    use crate::utils::serde::HasProxy;

    /// Fixture JSON shape `{ "major": <u64>, "minor": <u64> }`.
    #[derive(Deserialize)]
    pub struct ProtocolVersionProxy {
        major: u64,
        minor: u64,
    }

    impl From<ProtocolVersionProxy> for ProtocolVersion {
        fn from(p: ProtocolVersionProxy) -> Self {
            (p.major, p.minor)
        }
    }

    impl HasProxy for ProtocolVersion {
        type Proxy = ProtocolVersionProxy;
    }
}
