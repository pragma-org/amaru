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

use std::path::{Path, PathBuf};

use amaru_kernel::ProtocolVersion;

use crate::{Corpus, TestKey, read_corpus_root, read_test_data};

/// The `AMARU_TEST_FILTER` environment variable selects a subset by passing a substring of `<rule>/<category>/<file-name>`.
const TEST_FILTER: &str = "AMARU_TEST_FILTER";

/// The `AMARU_TEST_REPORT_DIRECTORY` environment variable names the directory where the run writes its results as JSON.
const TEST_REPORT_DIRECTORY: &str = "AMARU_TEST_REPORT_DIRECTORY";

pub struct TestConfiguration {
    root: PathBuf,
    filter: Option<String>,
    corpus: Corpus,
    protocol_version: ProtocolVersion,
    report_directory: Option<PathBuf>,
}

impl TestConfiguration {
    pub fn new(
        root: PathBuf,
        corpus: Corpus,
        protocol_version: ProtocolVersion,
        filter: Option<String>,
        report_directory: Option<PathBuf>,
    ) -> Self {
        Self { root, corpus, protocol_version, filter, report_directory }
    }

    pub fn create(protocol_version: ProtocolVersion) -> anyhow::Result<Self> {
        // only one corpus is supported for now
        let corpus = Corpus::Conway123_100;
        let root = read_corpus_root(corpus)?
            .ok_or_else(|| anyhow::anyhow!("corpus is missing; run `make fetch-cbor-dataset`"))?;
        let filter = std::env::var(TEST_FILTER).ok();
        let report_directory = std::env::var(TEST_REPORT_DIRECTORY)
            .ok()
            .map(|d| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(d));
        Ok(Self { root, corpus, protocol_version, filter, report_directory })
    }

    pub fn filter(&self) -> &Option<String> {
        &self.filter
    }

    pub fn report_directory(&self) -> Option<&Path> {
        self.report_directory.as_deref()
    }

    pub fn protocol_version(&self) -> ProtocolVersion {
        self.protocol_version
    }

    pub fn corpus(&self) -> Corpus {
        self.corpus
    }

    pub fn read_tests_for(&self, rule: &str) -> anyhow::Result<Vec<TestKey>> {
        let rule_dir = self.root.join(rule);
        assert!(rule_dir.is_dir(), "corpus is missing the rule directory {}", rule_dir.display());
        read_test_data(self.corpus, &rule_dir)
            .map(|tests| tests.into_iter().filter(|test_key| test_key.is_included(&self.filter)).collect())
    }
}
