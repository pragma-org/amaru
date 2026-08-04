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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    Normal,
    Maximized,
}

impl PaneMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::Normal => Self::Maximized,
            Self::Maximized => Self::Normal,
        }
    }

    pub fn is_maximized(self) -> bool {
        matches!(self, Self::Maximized)
    }
}
