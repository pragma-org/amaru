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

#[derive(Debug)]
pub struct HeaderValidationError {
    inner: anyhow::Error,
    missing_stake_distribution: Option<Epoch>,
}

impl HeaderValidationError {
    pub fn new(err: anyhow::Error) -> Self {
        HeaderValidationError { inner: err, missing_stake_distribution: None }
    }

    /// Construct an error indicating that the stake distribution for the given epoch is not
    /// yet available. This is a transient condition that callers may use to retry validation
    /// once the ledger has caught up.
    pub fn missing_stake_distribution(epoch: Epoch) -> Self {
        HeaderValidationError {
            inner: anyhow::anyhow!("no stake distribution available for pool access {}.", epoch),
            missing_stake_distribution: Some(epoch),
        }
    }

    /// If this error was caused by a missing stake distribution, return the epoch whose
    /// distribution is missing. Returns `None` for all other errors.
    pub fn as_missing_stake_distribution(&self) -> Option<Epoch> {
        self.missing_stake_distribution
    }

    pub fn to_anyhow(self) -> anyhow::Error {
        self.inner
    }

    pub fn downcast<T: std::error::Error + Debug + Send + Sync + 'static>(self) -> Result<T, anyhow::Error> {
        self.inner.downcast::<T>()
    }

    pub fn downcast_ref<T: std::error::Error + Debug + Send + Sync + 'static>(&self) -> Option<&T> {
        self.inner.downcast_ref::<T>()
    }
}

impl From<anyhow::Error> for HeaderValidationError {
    fn from(err: anyhow::Error) -> Self {
        HeaderValidationError::new(err)
    }
}

impl Display for HeaderValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeaderValidationError: {}", self.inner)
    }
}

impl Serialize for HeaderValidationError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.inner.to_string())
    }
}

/// This deserialization implementation is a best-effort attempt to
/// recover the error message. The original error type is lost during
/// serialization, so we can only reconstruct the error message as a string.
impl<'de> Deserialize<'de> for HeaderValidationError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(HeaderValidationError::new(anyhow::anyhow!(s)))
    }
}

impl PartialEq for HeaderValidationError {
    fn eq(&self, other: &Self) -> bool {
        self.inner.to_string() == other.inner.to_string()
            && self.missing_stake_distribution == other.missing_stake_distribution
    }
}
