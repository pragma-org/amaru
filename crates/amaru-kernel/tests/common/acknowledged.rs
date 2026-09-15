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

//! Failures the corpus is known to produce and that must not fail the run.
//!
//! A decoder that does not yet cover the whole corpus still has a conformance figure worth
//! publishing. Each entry here names one defect the suite is allowed to observe, so the run stays
//! green while the report keeps counting the failure. The report's `successful` flag is unaffected:
//! acknowledging a failure records that it is expected, not that the corpus conforms.

use std::{
    collections::BTreeSet,
    fmt::{self, Display},
    path::Path,
};

use serde::Deserialize;

/// One defect the suite may observe without failing.
///
/// `rule` and `class` together identify it: `class` is the collapsed failure reason, and pinning it
/// to a rule keeps the same defect in another rule from being excused silently.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub struct AcknowledgedFailure {
    pub rule: String,
    pub class: String,
    /// Why the failure is accepted and what would remove it.
    #[serde(default)]
    pub note: String,
}

impl Display for AcknowledgedFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} / {}", self.rule, self.class)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Declarations {
    #[serde(default)]
    acknowledged: Vec<AcknowledgedFailure>,
}

/// What a run accepts without failing.
#[derive(Debug, Clone)]
pub enum AcknowledgedFailures {
    /// No acknowledgement file has been created.
    None,
    /// Only these defects are accepted.
    These(Vec<AcknowledgedFailure>),
}

impl AcknowledgedFailures {
    /// Read the declarations, treating a missing file as "no failures accepted".
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Ok(Self::These(toml::from_str::<Declarations>(&contents)?.acknowledged)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::None),
            Err(e) => Err(e.into()),
        }
    }

    /// Return true if the run is allowed to observe a failure of the given rule and class.
    pub fn contains(&self, rule: &str, class: &str) -> bool {
        match self {
            Self::None => true,
            Self::These(entries) => entries.iter().any(|entry| entry.rule == rule && entry.class == class),
        }
    }

    /// Failures that the run did not observe
    pub fn stale(&self, observed: &BTreeSet<(String, String)>) -> Vec<AcknowledgedFailure> {
        match self {
            Self::None => Vec::new(),
            Self::These(entries) => entries
                .iter()
                .filter(|entry| !observed.contains(&(entry.rule.clone(), entry.class.clone())))
                .cloned()
                .collect(),
        }
    }
}
