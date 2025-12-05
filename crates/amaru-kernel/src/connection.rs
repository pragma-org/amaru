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

use crate::Point;
use parking_lot::Mutex;
use std::{fmt, sync::Arc};
use tokio::sync::oneshot;

pub type BlockSender = Arc<Mutex<Option<oneshot::Sender<Result<Vec<u8>, ClientConnectionError>>>>>;

#[allow(dead_code)]
pub enum ConnMsg {
    FetchBlock(Point, BlockSender),
    Disconnect,
}

#[derive(Debug)]
pub struct ClientConnectionError(anyhow::Error);

impl ClientConnectionError {
    pub fn new(err: anyhow::Error) -> Self {
        ClientConnectionError(err)
    }

    pub fn to_anyhow(self) -> anyhow::Error {
        self.0
    }

    pub fn downcast<T: std::error::Error + fmt::Debug + Send + Sync + 'static>(
        self,
    ) -> Result<T, anyhow::Error> {
        self.0.downcast::<T>()
    }

    pub fn downcast_ref<T: std::error::Error + fmt::Debug + Send + Sync + 'static>(
        &self,
    ) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }
}

impl From<anyhow::Error> for ClientConnectionError {
    fn from(err: anyhow::Error) -> Self {
        ClientConnectionError::new(err)
    }
}

impl fmt::Display for ClientConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClientConnectionError: {}", self.0)
    }
}

impl serde::Serialize for ClientConnectionError {
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
impl<'de> serde::Deserialize<'de> for ClientConnectionError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(ClientConnectionError::new(anyhow::anyhow!(s)))
    }
}

impl PartialEq for ClientConnectionError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}
