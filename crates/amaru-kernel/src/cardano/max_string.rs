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

use std::{fmt::Display, ops::Deref, str::FromStr};

use crate::cbor;

pub type MaxString128 = MaxString<128>;

/// A CDDL `text .size (0 .. 128)`: a URL or DNS name as carried on-chain.
///
/// Accepts the chunked encoding the node accepts, and enforces the length bound the CDDL states.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize, Default)]
#[repr(transparent)]
pub struct MaxString<const MAX: usize>(pub String);

impl<const MAX: usize> Deref for MaxString<MAX> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const MAX: usize> AsRef<str> for MaxString<MAX> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<const MAX: usize> Display for MaxString<MAX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<const MAX: usize> TryFrom<String> for MaxString<MAX> {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > MAX {
            Err(format!("string exceeds {} bytes: got {}", MAX, value.len()))
        } else {
            Ok(Self(value))
        }
    }
}

impl<const MAX: usize> FromStr for MaxString<MAX> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

impl<'b, C, const MAX: usize> cbor::Decode<'b, C> for MaxString<MAX> {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let text = cbor::decode_string(d)?;
        if text.len() > MAX {
            return Err(cbor::decode::Error::message(format!("text exceeds {MAX} bytes: got {}", text.len())));
        }
        Ok(Self(text.into_owned()))
    }
}

impl<C, const MAX: usize> cbor::Encode<C> for MaxString<MAX> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.str(&self.0)?.ok()
    }
}
