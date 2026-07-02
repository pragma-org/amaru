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

pub use pallas_primitives::conway::DRep;
use serde::ser::SerializeStruct;

use crate::StakeCredential;

#[derive(serde::Serialize)]
#[serde(transparent)]
pub struct AsJson<'a>(#[serde(serialize_with = "serialize")] pub &'a DRep);

pub fn serialize<S: serde::Serializer>(drep: &DRep, serializer: S) -> Result<S::Ok, S::Error> {
    match drep {
        DRep::Abstain => {
            let mut s = serializer.serialize_struct("drep", 1)?;
            s.serialize_field("type", "abstain")?;
            s
        }
        DRep::NoConfidence => {
            let mut s = serializer.serialize_struct("drep", 1)?;
            s.serialize_field("type", "no_confidence")?;
            s
        }
        DRep::Script(hash) => {
            let mut s = serializer.serialize_struct("drep", 2)?;
            s.serialize_field("type", "script")?;
            s.serialize_field("hash", &hex::encode(hash))?;
            s
        }
        DRep::Key(hash) => {
            let mut s = serializer.serialize_struct("drep", 2)?;
            s.serialize_field("type", "verification_key")?;
            s.serialize_field("hash", &hex::encode(hash))?;
            s
        }
    }
    .end()
}

pub fn to_stake_credential(drep: &DRep) -> Option<StakeCredential> {
    match drep {
        DRep::Key(hash) => Some(StakeCredential::AddrKeyhash(*hash)),
        DRep::Script(hash) => Some(StakeCredential::ScriptHash(*hash)),
        DRep::Abstain | DRep::NoConfidence => None,
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{DRep, any_hash28};

    pub fn any_drep() -> impl Strategy<Value = DRep> {
        prop_oneof![
            any_hash28().prop_map(DRep::Key),
            any_hash28().prop_map(DRep::Script),
            Just(DRep::Abstain),
            Just(DRep::NoConfidence),
        ]
    }
}
