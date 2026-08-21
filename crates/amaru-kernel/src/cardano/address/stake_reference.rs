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

use crate::{AddressPointer, AsHash, Credential};

/// The delegation part of a Shelley address: a stake credential, or a pointer to the
/// certificate that registered one. Enterprise addresses carry no [`StakeReference`] at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub enum StakeReference {
    Credential(Credential),
    Pointer(AddressPointer),
}

impl StakeReference {
    pub fn is_script(&self) -> bool {
        matches!(self, Self::Credential(credential) if credential.is_script())
    }

    /// The stake credential referenced, if the reference is a direct one.
    pub fn credential(&self) -> Option<Credential> {
        match self {
            Self::Credential(credential) => Some(*credential),
            Self::Pointer(..) => None,
        }
    }

    pub fn try_from_pointer(bytes: &[u8]) -> Option<Self> {
        AddressPointer::parse(bytes).map(Self::Pointer)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        match self {
            Self::Credential(credential) => credential.as_hash().to_vec(),
            Self::Pointer(ptr) => ptr.to_vec(),
        }
    }
}
