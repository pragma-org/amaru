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

/// Where a test sample sits in the corpus, which says what the suite must observe for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// Generated from the CDDL and accepted by the reference decoder. It must decode, and its re-encoding must
    /// match the reference bytes.
    Valid,
    /// Generated from the CDDL but rejected by the reference decoder: satisfying the CDDL is not enough to be
    /// decodable. It must be rejected. This is severity zero, the one severity that is not a mutation.
    InvalidGenerated,
    /// A CDDL-violating mutation of increasing severity. It must be rejected.
    Zapped(u8),
    /// Hand-written and accepted by the reference decoder. It must decode, and its re-encoding must match the
    /// reference bytes.
    ManualValid,
    /// Hand-written and rejected by the reference decoder. It must be rejected.
    ManualInvalid,
}

impl Category {
    /// Build a category from the name of the directory the samples sit in: `valid`, or `invalid-zap-<n>`.
    pub fn from_dir(name: &str) -> anyhow::Result<Self> {
        match name {
            "valid" => Ok(Category::Valid),
            "manual-valid" => Ok(Category::ManualValid),
            "manual-invalid" => Ok(Category::ManualInvalid),
            _ => name
                .strip_prefix("invalid-zap-")
                .and_then(|level| level.parse().ok())
                .map(|level| if level == 0 { Category::InvalidGenerated } else { Category::Zapped(level) })
                .ok_or_else(|| anyhow::anyhow!("unknown category type `{name}`")),
        }
    }
}

/// Displays as the path of the category inside its rule directory, which is also how the conformance report
/// names the category of a sample.
impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Category::Valid => write!(f, "valid"),
            Category::InvalidGenerated => write!(f, "invalid-zap-0"),
            Category::Zapped(level) => write!(f, "invalid-zap-{level}"),
            Category::ManualValid => write!(f, "manual-valid"),
            Category::ManualInvalid => write!(f, "manual-invalid"),
        }
    }
}
