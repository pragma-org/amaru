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

use std::{fmt::Display, path::PathBuf};

use anyhow::anyhow;

use crate::{Category, read_expected_canonical_cbor, read_file, sample_name};

/// A unique key for a test sample, consisting of the rule, category, and path to the sample file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TestKey {
    rule: String,
    category: Category,
    path: PathBuf,
}

impl TestKey {
    pub fn new(rule: String, category: Category, path: PathBuf) -> Self {
        Self { rule, category, path }
    }

    pub fn rule(&self) -> &str {
        &self.rule
    }

    pub fn is_included(&self, filter: &Option<String>) -> bool {
        filter.is_none() || filter.as_ref().is_some_and(|f| self.key().contains(f))
    }

    pub fn key(&self) -> String {
        let name = sample_name(&self.path).unwrap_or_default();
        format!("{}/{}/{}", self.rule, self.category, name)
    }

    pub fn read_bytes(&self) -> anyhow::Result<Vec<u8>> {
        read_file(&self.path)
    }

    pub fn category(&self) -> Category {
        self.category
    }

    /// The reference bytes the re-encoding of this sample must agree with. Only a `valid` sample has one, and
    /// every `valid` sample does; a missing one means the corpus is incomplete.
    pub fn read_expected_cbor(&self) -> anyhow::Result<Option<Vec<u8>>> {
        match self.category {
            Category::Valid | Category::ManualValid => Ok(Some(
                read_expected_canonical_cbor(&self.path)?
                    .ok_or_else(|| anyhow!("no reference bytes for the valid sample {self}"))?,
            )),
            Category::InvalidGenerated
            | Category::Zapped(_)
            | Category::ManualInvalid
            | Category::VerificationDeferred => Ok(None),
        }
    }
}

impl Display for TestKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}
