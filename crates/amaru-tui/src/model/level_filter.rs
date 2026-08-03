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

use tracing::Level;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelFilter {
    Debug,
    Info,
    Warn,
    Error,
}

impl LevelFilter {
    pub const ALL: [Self; 4] = [Self::Debug, Self::Info, Self::Warn, Self::Error];

    pub fn allows(self, level: Level) -> bool {
        match self {
            Self::Debug => matches!(level, Level::DEBUG | Level::INFO | Level::WARN | Level::ERROR),
            Self::Info => matches!(level, Level::INFO | Level::WARN | Level::ERROR),
            Self::Warn => matches!(level, Level::WARN | Level::ERROR),
            Self::Error => level == Level::ERROR,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}
