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

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bytes {
    pub bytes: Vec<u8>,
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bytes")
            .field("bytes", &hex::encode(&self.bytes))
            .finish()
    }
}

impl Serialize for Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_bytes(&self.bytes, serializer)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bytes(deserializer).map(|bytes| Bytes { bytes })
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Bytes { bytes }
    }
}

impl TryFrom<&str> for Bytes {
    type Error = hex::FromHexError;

    fn try_from(hex_string: &str) -> Result<Self, Self::Error> {
        Ok(Bytes {
            bytes: hex::decode(hex_string)?,
        })
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.bytes
    }
}

fn serialize_bytes<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let buf = hex::encode(bytes);
    serializer.serialize_str(&buf)
}

fn deserialize_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let bytes = hex::decode(buf).map_err(serde::de::Error::custom)?;
    Ok(bytes)
}
