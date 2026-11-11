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
pub const INPUT_SUFFIX: &str = ".input.cbor";

/// Suffix of the reference re-encoding of a sample, which sits beside it under the same name.
const EXPECTED_SUFFIX: &str = ".expected.cbor";

/// This files provides metadata about the cbor dataset, including how many samples it contains.
const COVERAGE_FILE: &str = "coverage.json";

/// Return every sample below a rule directory, ordered by category then file name so runs are reproducible.
///
/// A rule directory holds one directory per category: `valid`, and `invalid-zap-<n>` per mutation severity. A
/// severity directory can be missing, which the corpus means as zero samples at that severity.
pub fn read_test_data(rule_dir: &Path) -> anyhow::Result<Vec<TestKey>> {
    let mut out = Vec::new();
    let rule = normalize_rule_name(
        &rule_dir.file_name().ok_or_else(|| anyhow!("rule directory has no name"))?.to_string_lossy(),
    );
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
///
/// The totals declared by the corpus metadata are compared with the samples on disk. A mismatch is reported on
/// stderr rather than failing the run, since it is a defect of the dataset and not of amaru.
pub fn read_corpus_root(corpus: Corpus) -> anyhow::Result<Option<PathBuf>> {
    let root = corpus_root(corpus);
    if !root.is_dir() {
        return Ok(None);
    }
    check_no_unknown_rules(&root)?;
    for mismatch in coverage_mismatches(&read_coverage_totals(&root)?, &count_samples(&root)?) {
        eprintln!("warning: stale coverage metadata in {}: {mismatch}", root.join(COVERAGE_FILE).display());
    }
    Ok(Some(root))
}

/// Sample counts: either declared by the corpus metadata, or found on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleTotals {
    /// Every sample of the corpus.
    pub samples: u64,
    /// The hand-written samples, a subset of `samples`.
    pub manual: u64,
}

/// Read the sample totals declared by the corpus metadata.
pub fn read_coverage_totals(root: &Path) -> anyhow::Result<SampleTotals> {
    let path = root.join(COVERAGE_FILE);
    let coverage: serde_json::Value = serde_json::from_slice(&read_file(&path)?).context(path.display().to_string())?;
    let field =
        |name: &str| coverage[name].as_u64().ok_or_else(|| anyhow!("{} has no numeric `{name}` field", path.display()));
    Ok(SampleTotals { samples: field("samples")?, manual: field("manual")? })
}

/// Count the samples found below the rules of a corpus (including the manual ones).
pub fn count_samples(root: &Path) -> anyhow::Result<SampleTotals> {
    let mut totals = SampleTotals { samples: 0, manual: 0 };
    for rule in read_directory(root)?.into_iter().filter(|entry| entry.path().is_dir()) {
        for category in read_directory(&rule.path())?.into_iter().filter(|entry| entry.path().is_dir()) {
            let is_manual = category.file_name().to_string_lossy().starts_with("manual-");
            let count = read_directory(&category.path())?.iter().filter(|f| sample_name(&f.path()).is_some()).count();
            totals.samples += count as u64;
            if is_manual {
                totals.manual += count as u64;
            }
        }
    }
    Ok(totals)
}

/// Describe each way the declared totals disagree with the discovered ones. Empty when they agree.
pub fn coverage_mismatches(declared: &SampleTotals, discovered: &SampleTotals) -> Vec<String> {
    [("samples", declared.samples, discovered.samples), ("manual samples", declared.manual, discovered.manual)]
        .into_iter()
        .filter(|(_, declared, discovered)| declared != discovered)
        .map(|(name, declared, discovered)| format!("declares {declared} {name} but the corpus contains {discovered}"))
        .collect()
}

/// Normalize a corpus rule name escaped by Cygwin for a Windows filesystem.
///
/// Windows reserves several ASCII characters in filenames. Cygwin represents those characters by adding
/// `0xf000`, so the angle brackets in `%constr<plutus_data>` may be exposed as U+F03C and U+F03E.
pub fn normalize_rule_name(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            '\u{f022}' => '"',
            '\u{f02a}' => '*',
            '\u{f03a}' => ':',
            '\u{f03c}' => '<',
            '\u{f03e}' => '>',
            '\u{f03f}' => '?',
            '\u{f07c}' => '|',
            _ => character,
        })
        .collect()
}

/// Return the root of the corpus, e.g. `tests/data/cbor.dataset/conway`.
pub fn corpus_root(corpus: Corpus) -> PathBuf {
    dataset_dir().join(corpus.to_string())
}

/// Return the root of the dataset, e.g. `tests/cbor-dataset`.
pub fn dataset_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/cbor.dataset")
}

/// The commit of the dataset repository that the corpus is downloaded from, as pinned in `cbor.dataset.pin`.
pub fn dataset_commit() -> anyhow::Result<String> {
    let pin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/cbor.dataset.pin");
    let commit = String::from_utf8(read_file(&pin)?)?.trim().to_string();
    check_commit(&commit).context(format!("pin file {}", pin.display()))
}

/// Return a commit unchanged if it is a full hash: 40 lowercase hexadecimal digits.
pub fn check_commit(commit: &str) -> anyhow::Result<String> {
    let is_full_hash = commit.len() == 40 && commit.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if !is_full_hash {
        anyhow::bail!("`{commit}` is not a full commit hash (40 lowercase hexadecimal digits)");
    }
    Ok(commit.to_string())
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

#[cfg(test)]
mod tests {
    use super::{SampleTotals, coverage_mismatches, normalize_rule_name};

    #[test]
    fn matching_totals_have_no_mismatch() {
        let totals = SampleTotals { samples: 10, manual: 2 };
        assert!(coverage_mismatches(&totals, &totals).is_empty());
    }

    #[test]
    fn differing_totals_are_each_described() {
        let declared = SampleTotals { samples: 43430, manual: 1871 };
        let discovered = SampleTotals { samples: 43434, manual: 1875 };
        assert_eq!(
            coverage_mismatches(&declared, &discovered),
            vec![
                "declares 43430 samples but the corpus contains 43434".to_string(),
                "declares 1871 manual samples but the corpus contains 1875".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_cygwin_escaped_rule_name() {
        assert_eq!(normalize_rule_name("%constr\u{f03c}plutus_data\u{f03e}"), "%constr<plutus_data>");
    }
}
