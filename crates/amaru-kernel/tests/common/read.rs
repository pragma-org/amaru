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

/// Suffix of a sample file. One recursive scan of the corpus finds every sample by this suffix alone.
const INPUT_SUFFIX: &str = ".input.cbor";

/// Suffix of the reference re-encoding of a sample, which sits beside it under the same name.
const EXPECTED_SUFFIX: &str = ".expected.cbor";

/// Return every sample below a rule directory, ordered by category then file name so runs are reproducible.
///
/// A rule directory holds one directory per category: `valid`, and `invalid-zap-<n>` per mutation severity. A
/// severity directory can be missing, which the corpus means as zero samples at that severity.
pub fn read_test_data(rule_dir: &Path) -> anyhow::Result<Vec<TestKey>> {
    let mut out = Vec::new();
    let rule =
        rule_dir.file_name().ok_or_else(|| anyhow!("rule directory has no name"))?.to_string_lossy().into_owned();
    for entry in read_directory(rule_dir)? {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let category_dir = entry.file_name().to_string_lossy().into_owned();
        read_samples(&rule, &category_dir, &dir, &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// Collect the samples sitting directly in a category directory, leaving their reference re-encodings aside.
fn read_samples(rule: &str, category_dir: &str, dir: &Path, out: &mut Vec<TestKey>) -> anyhow::Result<()> {
    let category = Category::from_dir(category_dir).context(format!("directory {}", dir.display()))?;
    for file in read_directory(dir)? {
        let path = file.path();
        if sample_name(&path).is_some() {
            out.push(TestKey::new(rule.to_string(), category, path));
        }
    }
    Ok(())
}

/// The name of a sample, carrying no suffix, or `None` when the path is not a sample.
pub fn sample_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str()?.strip_suffix(INPUT_SUFFIX).map(|name| name.to_string())
}

/// Return the expected canonical CBOR for a given test sample, if it exists.
///
/// A reference is the sample's own path with `.input.cbor` replaced by `.expected.cbor`, so it is derived rather
/// than looked up in a parallel directory.
pub fn read_expected_canonical_cbor(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let name = sample_name(path).ok_or_else(|| anyhow!("{} is not a sample", path.display()))?;
    let at = path.with_file_name(format!("{name}{EXPECTED_SUFFIX}"));
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

/// Return the root of the corpus, e.g. `tests/data/cbor.dataset/conway`.
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
