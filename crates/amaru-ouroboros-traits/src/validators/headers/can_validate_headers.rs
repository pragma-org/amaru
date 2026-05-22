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

use std::fmt::{Debug, Display, Formatter};

use amaru_kernel::{BlockHeader, Epoch};
use serde::{Deserialize, Serialize};

pub trait CanValidateHeaders: Send + Sync {
    fn validate_header(&self, header: &BlockHeader) -> Result<(), HeaderValidationError>;
}

/// An error reported by header validation.
///
/// The `MissingStakeDistribution` variant flags a transient condition: the ledger has not yet
/// computed the stake distribution required to validate this header. In that case we need to wait
/// until the ledger has caught up to revalidate the header.
#[derive(Debug)]
pub enum HeaderValidationError {
    MissingStakeDistribution(Epoch),
    Other(anyhow::Error),
}

impl HeaderValidationError {
    /// If this error was caused by a missing stake distribution, return the epoch whose
    /// distribution is missing. Returns `None` for all other errors.
    pub fn as_missing_stake_distribution(&self) -> Option<Epoch> {
        match self {
            Self::MissingStakeDistribution(epoch) => Some(*epoch),
            Self::Other(_) => None,
        }
    }
}

impl From<anyhow::Error> for HeaderValidationError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl Display for HeaderValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingStakeDistribution(epoch) => {
                write!(f, "HeaderValidationError: no stake distribution available for epoch {}.", epoch)
            }
            Self::Other(err) => write!(f, "HeaderValidationError: {}", err),
        }
    }
}

impl Serialize for HeaderValidationError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Best-effort deserialization: the original error type is lost during serialization, so
/// any recovered value lands in the `Other` variant carrying the error message as a string.
impl<'de> Deserialize<'de> for HeaderValidationError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::Other(anyhow::anyhow!(s)))
    }
}

impl PartialEq for HeaderValidationError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MissingStakeDistribution(a), Self::MissingStakeDistribution(b)) => a == b,
            (Self::Other(a), Self::Other(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}
