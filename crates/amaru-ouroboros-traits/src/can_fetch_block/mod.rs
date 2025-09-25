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

use amaru_kernel::Point;
use amaru_kernel::peer::Peer;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};

pub mod mock;

#[async_trait]
pub trait CanFetchBlock: Send + Sync {
    async fn fetch_block(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> Result<Vec<u8>, BlockFetchClientError>;
}

#[derive(Debug)]
pub struct BlockFetchClientError(anyhow::Error);

impl BlockFetchClientError {
    pub fn new(err: anyhow::Error) -> Self {
        BlockFetchClientError(err)
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

impl From<anyhow::Error> for BlockFetchClientError {
    fn from(err: anyhow::Error) -> Self {
        BlockFetchClientError::new(err)
    }
}

impl Display for BlockFetchClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlockFetchClientError: {}", self.0)
    }
}

impl Serialize for BlockFetchClientError {
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
impl<'de> Deserialize<'de> for BlockFetchClientError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(BlockFetchClientError::new(anyhow::anyhow!(s)))
    }
}

impl PartialEq for BlockFetchClientError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}
