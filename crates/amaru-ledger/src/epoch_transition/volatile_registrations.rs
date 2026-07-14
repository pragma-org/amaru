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

use std::collections::BTreeSet;

use amaru_kernel::StakeCredential;

/// This data type tracks the accounts registered and deregistered in a volatile view.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VolatileRegistrations<'volatile> {
    /// Accounts registered in the volatile view
    registered: BTreeSet<&'volatile StakeCredential>,

    /// Accounts deregistered in the volatile view
    deregistered: BTreeSet<&'volatile StakeCredential>,
}

impl<'volatile> VolatileRegistrations<'volatile> {
    pub fn new(
        registered: BTreeSet<&'volatile StakeCredential>,
        deregistered: BTreeSet<&'volatile StakeCredential>,
    ) -> Self {
        Self { registered, deregistered }
    }

    /// Return the most recent registration status in the volatile window.
    pub fn latest_registration(&self, credential: &StakeCredential) -> VolatileRegistrationStatus {
        if self.registered.contains(credential) {
            VolatileRegistrationStatus::Registered
        } else if self.deregistered.contains(credential) {
            VolatileRegistrationStatus::Unregistered
        } else {
            VolatileRegistrationStatus::Unknown
        }
    }

    pub fn unregistered(&self) -> impl Iterator<Item = &StakeCredential> {
        self.deregistered.iter().copied()
    }

    pub fn is_registered(&self, credential: &StakeCredential) -> bool {
        self.registered.contains(credential)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum VolatileRegistrationStatus {
    Registered,
    Unregistered,
    Unknown,
}
