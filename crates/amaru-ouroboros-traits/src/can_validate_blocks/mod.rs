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

use crate::BlockHeader;
use amaru_kernel::{Point, RawBlock};
use amaru_metrics::ledger::LedgerMetrics;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};

pub mod mock;

#[async_trait::async_trait]
pub trait CanValidateBlocks: Send + Sync {
    async fn roll_forward_block(
        &self,
        point: &Point,
        block: &RawBlock,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>;

    fn rollback_block(&self, to: &Point) -> Result<(), BlockValidationError>;
}
#[derive(Debug)]
pub struct BlockValidationError(anyhow::Error);

impl BlockValidationError {
    pub fn new(err: anyhow::Error) -> Self {
        BlockValidationError(err)
    }

    pub fn to_anyhow(self) -> anyhow::Error {
        self.0
    }

    pub fn downcast<T: std::error::Error + Debug + Send + Sync + 'static>(
        self,
    ) -> Result<T, anyhow::Error> {
        self.0.downcast::<T>()
    }

    pub fn downcast_ref<T: std::error::Error + Debug + Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }
}

impl From<anyhow::Error> for BlockValidationError {
    fn from(err: anyhow::Error) -> Self {
        BlockValidationError::new(err)
    }
}

impl Display for BlockValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlockValidationError: {}", self.0)
    }
}

impl Serialize for BlockValidationError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// This deserialization implementation is a best-effort attempt to
/// recover the error message. The original error type is lost during
/// serialization, so we can only reconstruct the error message as a string.
impl<'de> Deserialize<'de> for BlockValidationError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(BlockValidationError::new(anyhow::anyhow!(s)))
    }
}

impl PartialEq for BlockValidationError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}

pub trait CanValidateHeaders: Send + Sync {
    fn validate_header(
        &self,
        point: &Point,
        header: &BlockHeader,
    ) -> Result<(), HeaderValidationError>;
}
#[derive(Debug)]
pub struct HeaderValidationError(anyhow::Error);

impl HeaderValidationError {
    pub fn new(err: anyhow::Error) -> Self {
        HeaderValidationError(err)
    }

    pub fn to_anyhow(self) -> anyhow::Error {
        self.0
    }

    pub fn downcast<T: std::error::Error + Debug + Send + Sync + 'static>(
        self,
    ) -> Result<T, anyhow::Error> {
        self.0.downcast::<T>()
    }

    pub fn downcast_ref<T: std::error::Error + Debug + Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }
}

impl From<anyhow::Error> for HeaderValidationError {
    fn from(err: anyhow::Error) -> Self {
        HeaderValidationError::new(err)
    }
}

impl Display for HeaderValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeaderValidationError: {}", self.0)
    }
}

impl Serialize for HeaderValidationError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
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
        self.0.to_string() == other.0.to_string()
    }
}
