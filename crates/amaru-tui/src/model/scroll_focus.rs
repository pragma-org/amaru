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

use super::Page;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollFocus {
    Logs,
    Peers,
    Proposals,
}

impl ScrollFocus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::Peers => "peers",
            Self::Proposals => "proposals",
        }
    }

    pub fn next_for(self, page: Page) -> Self {
        match (page, self) {
            (Page::Amaru, Self::Logs) => Self::Peers,
            (Page::Amaru, Self::Peers) => Self::Logs,
            (Page::Amaru, Self::Proposals) => Self::Logs,
            (Page::Cardano, Self::Logs) => Self::Proposals,
            (Page::Cardano, Self::Proposals) => Self::Logs,
            (Page::Cardano, Self::Peers) => Self::Logs,
            (Page::Config, focus) => focus,
        }
    }

    pub fn previous_for(self, page: Page) -> Self {
        match (page, self) {
            (Page::Amaru, Self::Logs) => Self::Peers,
            (Page::Amaru, Self::Peers) => Self::Logs,
            (Page::Amaru, Self::Proposals) => Self::Logs,
            (Page::Cardano, Self::Logs) => Self::Proposals,
            (Page::Cardano, Self::Proposals) => Self::Logs,
            (Page::Cardano, Self::Peers) => Self::Logs,
            (Page::Config, focus) => focus,
        }
    }
}
