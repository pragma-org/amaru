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

use std::{fmt, fmt::Debug, str::FromStr};

use crate::cbor;

pub const PROTOCOL_VERSION_10: ProtocolVersion = ProtocolVersion::new(10, 0);

pub const PROTOCOL_VERSION_11: ProtocolVersion = ProtocolVersion::new(11, 0);

pub const PROTOCOL_VERSION_12: ProtocolVersion = ProtocolVersion::new(12, 0);

pub const DEFAULT: ProtocolVersion = PROTOCOL_VERSION_11;

pub const MINIMUM_SUPPORTED: ProtocolVersion = PROTOCOL_VERSION_10;

/// A Cardano protocol version, as committed in block headers and in protocol parameters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct ProtocolVersion {
    major: u64,
    minor: u64,
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        DEFAULT
    }
}

impl Debug for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.major, self.minor)
    }
}

impl ProtocolVersion {
    /// Highest major version the ledger recognises.
    ///
    /// See <https://github.com/IntersectMBO/cardano-ledger/blob/9f6b6f1ab10d7cc730dae3328f4003e7fa55afe2/eras/conway/impl/cddl/data/conway.cddl#L105>
    const MAX_MAJOR: u64 = PROTOCOL_VERSION_12.major();

    pub const fn new(major: u64, minor: u64) -> Self {
        Self { major, minor }
    }

    pub const fn major(&self) -> u64 {
        self.major
    }

    pub const fn minor(&self) -> u64 {
        self.minor
    }

    pub fn can_follow(&self, other: ProtocolVersion) -> bool {
        (self.major == other.major() + 1 && self.minor == 0)
            || (self.major == other.major() && self.minor == other.minor() + 1)
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for ProtocolVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &s.split(".").collect::<Vec<_>>()[..] {
            [major, minor] => {
                let major = major.parse::<u64>().map_err(|e| e.to_string())?;
                let minor = minor.parse::<u64>().map_err(|e| e.to_string())?;
                Ok(Self::new(major, minor))
            }
            _ => Err(s.to_string()),
        }
    }
}

impl<C> cbor::Encode<C> for ProtocolVersion {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.u64(self.major)?;
        e.u64(self.minor)?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for ProtocolVersion {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let major = d.u64()?;
            if major > Self::MAX_MAJOR {
                return Err(cbor::decode::Error::message("invalid protocol version's major: too high"));
            }
            let minor = d.u64()?;
            Ok(Self::new(major, minor))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("protocol version {}.{} is too old; minimum supported version is {}.{}", snapshot_version.major(), snapshot_version.minor(), minimum_version.major(), minimum_version.minor())]
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

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::ProtocolVersion;

    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(11, 0) => true; "next major version")]
    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(10, 3) => true; "next minor version")]
    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(10, 2) => false; "same version")]
    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(11, 1) => false; "next major version with nonzero minor")]
    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(12, 0) => false; "skipped major version")]
    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(10, 4) => false; "skipped minor version")]
    #[test_case(ProtocolVersion::new(10, 2), ProtocolVersion::new(9, 0) => false; "older version")]
    fn can_follow(current: ProtocolVersion, new: ProtocolVersion) -> bool {
        new.can_follow(current)
    }
}
