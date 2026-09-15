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

use std::fmt;

/// Which expectation a sample carries, derived from its directory rather than from metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// Must decode.
    CddlGenerated,
    /// A CDDL-violating mutation of increasing severity; must be rejected.
    Zapped(u8),
}

impl Category {
    pub fn from_dir(name: &str) -> anyhow::Result<Self> {
        match name {
            "valid" => Ok(Category::CddlGenerated),
            _ => name
                .strip_prefix("zap-")
                .and_then(|level| level.parse().ok())
                .map(Category::Zapped)
                .ok_or_else(|| anyhow::anyhow!("unknown category type `{name}`")),
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Category::CddlGenerated => write!(f, "valid"),
            Category::Zapped(level) => write!(f, "zap-{level}"),
        }
    }
}
