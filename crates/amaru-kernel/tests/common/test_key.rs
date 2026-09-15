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

use crate::{
    Category,
    Category::{CddlGenerated, Zapped},
    Corpus, read_expected_canonical_cbor, read_file,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TestKey {
    corpus: Corpus,
    rule: String,
    category: Category,
    path: PathBuf,
}

impl TestKey {
    pub fn new(corpus: Corpus, rule: String, category: Category, path: PathBuf) -> Self {
        Self { corpus, rule, category, path }
    }

    pub fn rule(&self) -> &str {
        &self.rule
    }

    pub fn is_included(&self, filter: &Option<String>) -> bool {
        filter.is_none() || filter.as_ref().is_some_and(|f| self.key().contains(f))
    }

    pub fn key(&self) -> String {
        let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{}/{}/{}", self.rule, self.category, stem)
    }

    pub fn read_bytes(&self) -> anyhow::Result<Vec<u8>> {
        read_file(&self.path)
    }

    pub fn read_expected_cbor(&self) -> anyhow::Result<Option<Vec<u8>>> {
        match self.category {
            CddlGenerated => read_expected_canonical_cbor(self.corpus, &self.rule, &self.path),
            Zapped(_) => Ok(None),
        }
    }

    pub fn is_zapped(&self) -> bool {
        matches!(self.category, Zapped(_))
    }
}

impl Display for TestKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}
