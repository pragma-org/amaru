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

#[derive(Debug, PartialEq, Copy, Clone)]
pub struct MachineVersion {
    pub major: usize,
    pub minor: usize,
    pub patch: usize,
}

impl Default for MachineVersion {
    fn default() -> Self {
        Self::V1_1_0
    }
}

impl MachineVersion {
    pub const V1_0_0: Self = Self::new(1, 0, 0);
    pub const V1_1_0: Self = Self::new(1, 1, 0);

    pub const fn new(major: usize, minor: usize, patch: usize) -> Self {
        Self { major, minor, patch }
    }

    pub fn is_constr_case_available(&self) -> bool {
        (self.major, self.minor) >= (1, 1)
    }
}
