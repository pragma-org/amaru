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

use crate::{AcknowledgedFailures, Corpus, TestKey, corpus_root, read_corpus_root, read_test_data};

/// The `AMARU_TEST_FILTER` environment variable selects a subset by passing a substring of `<rule>/<category>/<file-name>`.
const TEST_FILTER: &str = "AMARU_TEST_FILTER";

/// The `AMARU_TEST_REPORT_DIRECTORY` environment variable names the directory where the run writes its results as JSON.
const TEST_REPORT_DIRECTORY: &str = "AMARU_TEST_REPORT_DIRECTORY";

/// Where the acknowledged failures are declared, relative to the crate root.
const ACKNOWLEDGED_FAILURES_FILE: &str = "tests/acknowledged-failures.toml";

pub struct TestConfiguration {
    /// The root of the corpus, e.g. `tests/cbor-dataset`.
    root: PathBuf,
    /// The substring filter to select a subset of the corpus, e.g. `block/valid`.
    filter: Option<String>,
    /// The corpus to test against, e.g. `Conway123_100`.
    corpus: Corpus,
    /// The protocol version to use for the round-trip tests.
    protocol_version: ProtocolVersion,
    /// The directory where the run writes its results as JSON, if any.
    report_directory: Option<PathBuf>,
    /// The failures the run is allowed to observe without failing.
    acknowledged_failures: AcknowledgedFailures,
    /// The file where the acknowledged failures are declared.
    acknowledged_failures_file: PathBuf,
}

impl TestConfiguration {
    /// Create a new test configuration for the given protocol version.
    pub fn create(protocol_version: ProtocolVersion) -> anyhow::Result<Option<Self>> {
        // only one corpus is supported for now
        let corpus = Corpus::Conway123_100;
        let report_directory =
            std::env::var(TEST_REPORT_DIRECTORY).ok().map(|d| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(d));
        let acknowledged_failures_file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(ACKNOWLEDGED_FAILURES_FILE);
        let acknowledged_failures = AcknowledgedFailures::read(&acknowledged_failures_file)?;

        // Read the test data. If the results were expected to be exported and the corpus is missing
        // raise an error, otherwise just return None to skip the test.
        let Some(root) = read_corpus_root(corpus)? else {
            if report_directory.is_some() {
                anyhow::bail!(
                    "corpus {} is missing in {}; set {} to a directory to write the report anyway",
                    corpus,
                    corpus_root(corpus).display(),
                    TEST_REPORT_DIRECTORY
                );
            } else {
                return Ok(None);
            }
        };
        let filter = std::env::var(TEST_FILTER).ok();
        Ok(Some(Self {
            root,
            corpus,
            protocol_version,
            filter,
            report_directory,
            acknowledged_failures,
            acknowledged_failures_file,
        }))
    }

    pub fn acknowledged_failures(&self) -> &AcknowledgedFailures {
        &self.acknowledged_failures
    }

    pub fn acknowledged_failures_file(&self) -> &Path {
        &self.acknowledged_failures_file
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

    /// Read the tests keys for a given rule, filtering them by the substring filter if any.
    pub fn read_tests_for(&self, rule: &str) -> anyhow::Result<Vec<TestKey>> {
        let rule_dir = self.root.join(rule);
        assert!(rule_dir.is_dir(), "corpus is missing the rule directory {}", rule_dir.display());
        read_test_data(self.corpus, &rule_dir)
            .map(|tests| tests.into_iter().filter(|test_key| test_key.is_included(&self.filter)).collect())
    }
}
