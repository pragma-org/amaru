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

use crate::{AsIndex, Hash, MemoizedDatum, RedeemerKey, ScriptPurpose, size::SCRIPT};
use std::cmp::Ordering;

#[derive(Clone, Eq, PartialEq, Debug, serde::Deserialize)]
pub struct RequiredScript {
    pub hash: Hash<SCRIPT>,
    pub index: u32,
    pub purpose: ScriptPurpose,
    pub datum: MemoizedDatum,
}

impl PartialOrd for RequiredScript {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&RequiredScript> for Hash<SCRIPT> {
    fn from(value: &RequiredScript) -> Self {
        value.hash
    }
}

impl Ord for RequiredScript {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.hash.cmp(&other.hash) {
            Ordering::Equal => match self.purpose.as_index().cmp(&other.purpose.as_index()) {
                Ordering::Equal => self.index.cmp(&other.index),
                by_purpose @ Ordering::Less | by_purpose @ Ordering::Greater => by_purpose,
            },
            by_hash @ Ordering::Less | by_hash @ Ordering::Greater => by_hash,
        }
    }
}

impl From<&RequiredScript> for RedeemerKey {
    fn from(value: &RequiredScript) -> Self {
        RedeemerKey {
            tag: value.purpose,
            index: value.index,
        }
    }
}
