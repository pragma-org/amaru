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
pub enum Page {
    Amaru,
    Cardano,
    Config,
}

impl Page {
    pub const ALL: [Self; 3] = [Self::Amaru, Self::Cardano, Self::Config];

    pub fn next(self) -> Self {
        match self {
            Self::Amaru => Self::Cardano,
            Self::Cardano => Self::Config,
            Self::Config => Self::Amaru,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Amaru => Self::Config,
            Self::Cardano => Self::Amaru,
            Self::Config => Self::Cardano,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Amaru => "Amaru",
            Self::Cardano => "Cardano",
            Self::Config => "Config",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Amaru => 0,
            Self::Cardano => 1,
            Self::Config => 2,
        }
    }
}
