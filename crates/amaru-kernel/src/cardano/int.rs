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

use std::ops::Deref;

use crate::cbor;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    std::hash::Hash,
    serde::Serialize,
    serde::Deserialize,
    cbor::Encode,
    cbor::Decode,
)]
#[cbor(transparent)]
#[serde(into = "i128")]
#[serde(try_from = "i128")]
pub struct Int(#[n(0)] pub cbor::data::Int);

impl Deref for Int {
    type Target = cbor::data::Int;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Int> for i128 {
    fn from(value: Int) -> Self {
        i128::from(value.0)
    }
}

impl From<i64> for Int {
    fn from(x: i64) -> Self {
        let inner = cbor::data::Int::from(x);
        Self(inner)
    }
}

impl TryFrom<i128> for Int {
    type Error = cbor::data::TryFromIntError;

    fn try_from(value: i128) -> Result<Self, Self::Error> {
        let inner = cbor::data::Int::try_from(value)?;
        Ok(Self(inner))
    }
}
