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

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};

use crate::{Category, Corpus, TestKey, check_no_unknown_rules};

/// Directory holding the canonical re-encoding of the samples it accompanies. It is not a sample category, so
/// it is skipped when the samples of a rule are collected.
const EXPECTED_DIR: &str = "expected";

/// Directory grouping the samples that must be rejected. It holds one sub-directory per mutation severity,
/// `zap-0` being the samples the generator produced unaltered and the decoder still rejects.
const INVALID_DIR: &str = "invalid";

/// Return every `.cbor` sample below a rule directory, ordered by category then file name so runs are
/// reproducible.
///
/// A rule directory holds `valid`, `expected` and `invalid`, the last one nesting a directory per severity.
pub fn read_test_data(corpus: Corpus, rule_dir: &Path) -> anyhow::Result<Vec<TestKey>> {
    let mut out = Vec::new();
    let rule =
        rule_dir.file_name().ok_or_else(|| anyhow!("rule directory has no name"))?.to_string_lossy().into_owned();
    for entry in read_directory(rule_dir)? {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        match entry.file_name().to_string_lossy().as_ref() {
            EXPECTED_DIR => continue,
            INVALID_DIR => {
                for severity in read_directory(&dir)? {
                    let severity_dir = severity.path();
                    if severity_dir.is_dir() {
                        let name = severity.file_name().to_string_lossy().into_owned();
                        read_samples(corpus, &rule, &name, &severity_dir, &mut out)?;
                    }
                }
            }
            name => read_samples(corpus, &rule, name, &dir, &mut out)?,
        }
    }
    out.sort();
    Ok(out)
}

/// Collect the `.cbor` samples sitting directly in a category directory.
fn read_samples(
    corpus: Corpus,
    rule: &str,
    category_dir: &str,
    dir: &Path,
    out: &mut Vec<TestKey>,
) -> anyhow::Result<()> {
    let category = Category::from_dir(category_dir).context(format!("directory {}", dir.display()))?;
    for file in read_directory(dir)? {
        let path = file.path();
        if path.extension().is_some_and(|ext| ext == "cbor") {
            out.push(TestKey::new(corpus, rule.to_string(), category, path));
        }
    }
    Ok(())
}

/// Return the expected canonical CBOR for a given test sample, if it exists.
///
/// A reference lives at `<corpus>/<rule>/expected/<sample>`, next to the categories of the rule it belongs to,
/// and carries the same file name as the sample it is the re-encoding of.
pub fn read_expected_canonical_cbor(corpus: Corpus, rule: &str, path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let at = corpus_root(corpus).join(rule).join(EXPECTED_DIR).join(path.file_name().unwrap_or_default());
    at.is_file().then(|| read_file(&at)).transpose()
}

/// Return the root of the corpus, if it exists, and check that there are no unknown rules.
pub fn read_corpus_root(corpus: Corpus) -> anyhow::Result<Option<PathBuf>> {
    let root = corpus_root(corpus);
    if !root.is_dir() {
        return Ok(None);
    }
    check_no_unknown_rules(&root)?;
    Ok(Some(root))
}

/// Return the root of the corpus, e.g. `tests/cbor-dataset/Conway123_100`.
pub fn corpus_root(corpus: Corpus) -> PathBuf {
    dataset_dir().join(corpus.to_string())
}

/// Return the root of the dataset, e.g. `tests/cbor-dataset`.
pub fn dataset_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/cbor.dataset")
}

/// Return the contents of a file
pub fn read_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    fs::read(path).map_err(|e| anyhow!(io_error(path, e)))
}

/// Return the directory entries
pub fn read_directory(path: &Path) -> anyhow::Result<Vec<fs::DirEntry>> {
    fs::read_dir(path)
        .map_err(|e| anyhow!(io_error(path, e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow!(io_error(path, e)))
}

fn io_error(path: &Path, e: std::io::Error) -> String {
    format!("read {}: {e}", path.display())
}
