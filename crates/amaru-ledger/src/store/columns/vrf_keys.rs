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

use amaru_kernel::{Hash, size::VRF_KEY};

pub type Key = Hash<VRF_KEY>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffVrf {
    /// Mark a VRF key hash as in use by a pool registration. This *sets* the count to 1 even when
    /// an entry alread exists.
    Claim,
    /// Delete a specific  retiring pools' hold on a VRF key hash, deleting the entry once nothing
    /// holds it.
    Release,
    /// Remove a specific amount for a key counter.
    Decrement(u64),
}

impl DiffVrf {
    pub fn then(&mut self, next: Self) {
        match next {
            Self::Claim | Self::Release => *self = next,
            Self::Decrement(n_next) => match self {
                Self::Release => {}
                Self::Claim => *self = Self::Release,
                Self::Decrement(n_self) => *self = Self::Decrement(*n_self + n_next),
            },
        }
    }
}
