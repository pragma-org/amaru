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

use amaru_kernel::{ProtocolVersion, protocol_version};

pub const MINIMUM_SUPPORTED_PROTOCOL_VERSION: ProtocolVersion = protocol_version::PROTOCOL_VERSION_10;

#[derive(Debug, thiserror::Error)]
#[error("protocol version {snapshot_version} is too old; minimum supported version is {minimum_version}")]
pub struct ProtocolVersionTooOld {
    pub snapshot_version: String,
    pub minimum_version: String,
}

pub fn validate_protocol_version(
    version: ProtocolVersion,
    minimum_version: ProtocolVersion,
) -> Result<(), ProtocolVersionTooOld> {
    if version.0 < minimum_version.0 || (version.0 == minimum_version.0 && version.1 < minimum_version.1) {
        return Err(ProtocolVersionTooOld {
            snapshot_version: protocol_version::fmt(&version),
            minimum_version: protocol_version::fmt(&minimum_version),
        });
    }

    Ok(())
}
