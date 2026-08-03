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
    env, fs,
    path::{Path, PathBuf},
};

use amaru_kernel::{cbor, from_cbor_no_leftovers, to_cbor, Block, EraName, TransactionBody};
use serde::Deserialize;

/// See the README at crates/amaru/tests/conformance/serialization/cbor-fixture-generator/README.md
/// to regenerate fixtures.
///
/// You can run this specific test with:
/// ```
/// cargo test -p amaru-kernel --test test_cbor_serialization -- --no-capture
/// ```
///
/// And use the environment variable `AMARU_FIXTURE_FILTER` to only run a specific test
/// (a substring of the file name is enough to select it):
/// ```
/// export AMARU_FIXTURE_FILTER="0df40008"
/// ```
///
#[test]
fn test_cbor_serialization() {
    let mut failures: Vec<String> = Vec::new();
    let fixtures = collect_fixtures(&mut failures);
    assert!(!fixtures.is_empty(), "expected at least one fixture under {}", fixtures_root().display());

    // We count the number of well-formed vs non well-formed expectations,
    // plus how many fixtures are flagged as non-conformant
    // (known_amaru_divergence) — the latter is a live signal that should drop
    // toward zero as amaru's decoders / encoders are aligned with the CDDL.
    let mut positive = 0_usize;
    let mut negative = 0_usize;
    let mut on_chain_drift: Vec<&Fixture> = Vec::new();
    let mut cuddle_rejected: Vec<&Fixture> = Vec::new();
    let mut antigen_accepted: Vec<&Fixture> = Vec::new();
    let mut other_divergence: Vec<&Fixture> = Vec::new();
    for fixture in &fixtures {
        if fixture.expectations.well_formed {
            positive += 1;
        } else {
            negative += 1;
        }
        if fixture.expectations.known_amaru_divergence {
            match fixture.expectations.source.as_deref() {
                None | Some("on-chain") => on_chain_drift.push(fixture),
                Some("cuddle") => cuddle_rejected.push(fixture),
                Some("antigen") => antigen_accepted.push(fixture),
                Some(_) => other_divergence.push(fixture),
            }
        }
        if let Err(msg) = run_test(fixture) {
            failures.push(format!("{}: {}", fixture.path.display(), msg));
        }
    }

    let non_conformant = on_chain_drift.len() + cuddle_rejected.len() + antigen_accepted.len() + other_divergence.len();
    if non_conformant > 0 {
        eprintln!("non-conformant cases:");
        print_divergence_list(&on_chain_drift, "on-chain edge case");
        print_divergence_list(&cuddle_rejected, "cuddle positive amaru-rejects");
        print_divergence_list(&antigen_accepted, "antigen negative amaru-accepts");
        print_divergence_list(&other_divergence, "other");
        eprintln!();
    }
    eprintln!(
        "cbor fixtures: {} positive, {} negative, {} total ({} non-conformant)",
        positive,
        negative,
        fixtures.len(),
        non_conformant,
    );
    if non_conformant > 0 {
        eprintln!("  non-conformant breakdown:");
        eprintln!("    count  reason");
        if !on_chain_drift.is_empty() {
            eprintln!(
                "    {:>5}  on-chain edge case — encoder round-trip drift (e.g. AuxiliaryData TAG_MAP_259 stripped, non-canonical encodings)",
                on_chain_drift.len()
            );
        }
        if !cuddle_rejected.is_empty() {
            eprintln!(
                "    {:>5}  cuddle positive — amaru's decoder rejects CDDL-valid input (e.g. address header validation, narrower integer widths, indefinite-length encodings)",
                cuddle_rejected.len()
            );
        }
        if !antigen_accepted.is_empty() {
            eprintln!(
                "    {:>5}  antigen negative — amaru's decoder accepts CDDL-invalid input (e.g. zapAntiGen produced bytes amaru still parses)",
                antigen_accepted.len()
            );
        }
        if !other_divergence.is_empty() {
            eprintln!("    {:>5}  other / unknown source", other_divergence.len());
        }
    }

    if !failures.is_empty() {
        panic!("{} fixture(s) failed:\n  - {}", failures.len(), failures.join("\n  - "));
    }
}

/// Walk the data tree and yield every directory that contains both
/// `sample.cbor` and `meta.json`. The fixture kind is taken from the top-level
/// directory name under `cbor.decode/` (e.g. `block`, `transaction_body`).
///
/// IO and parse errors are collected into `errors` rather than panicking.
fn collect_fixtures(errors: &mut Vec<String>) -> Vec<Fixture> {
    let root = fixtures_root();
    let mut out = Vec::new();
    let Ok(top) = fs::read_dir(&root) else {
        return out;
    };
    for entry in top.flatten() {
        let kind_dir = entry.path();
        let Some(name) = kind_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(kind) = Kind::from_root(name) else {
            continue;
        };

        load_fixtures(&kind_dir, kind, &mut out, errors);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Recursively collect fixtures from files of a given kind
fn load_fixtures(dir: &Path, kind: Kind, out: &mut Vec<Fixture>, errors: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let filter: Option<String> = env::var("AMARU_FIXTURE_FILTER").ok();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let sample = path.join("sample.cbor");
        let meta = path.join("meta.json");
        if sample.is_file() && meta.is_file() {
            let must_load = filter.as_ref().is_none_or(|f| entry.file_name().to_string_lossy().contains(f));
            if must_load {
                match load_fixture(path, kind, &sample, &meta) {
                    Ok(fx) => out.push(fx),
                    Err(e) => errors.push(e),
                }
            }
        } else {
            load_fixtures(&path, kind, out, errors);
        }
    }
}

/// Read a fixture file at a given path
fn load_fixture(path: PathBuf, kind: Kind, sample: &Path, meta: &Path) -> Result<Fixture, String> {
    let bytes = fs::read(sample).map_err(|e| format!("read {}: {e}", sample.display()))?;
    let meta_str = fs::read_to_string(meta).map_err(|e| format!("read {}: {e}", meta.display()))?;
    let meta_value: Expectations =
        serde_json::from_str(&meta_str).map_err(|e| format!("parse {}: {e}", meta.display()))?;
    Ok(Fixture { path, kind, expectations: meta_value, bytes })
}

/// Run a decoding / encoding test with the given fixture.
///
/// When `known_amaru_divergence` is true, a failure is pardoned — but we also
/// verify the flag is *not stale*: if amaru's behavior agrees with the labelled
/// `well_formed` expectation, the flag should be removed and the test fails
/// asking the caller (or the regenerate script) to drop it.
fn run_test(fixture: &Fixture) -> Result<(), String> {
    let divergent = fixture.expectations.known_amaru_divergence;
    let result = match fixture.kind {
        Kind::Block => check_block(&fixture.bytes),
        Kind::TransactionBody => check_transaction_body(&fixture.bytes),
    };
    let stale_flag_msg = "stale known_amaru_divergence flag — amaru now agrees with the labelled expectation; remove the flag from meta.json";
    match (fixture.expectations.well_formed, result, divergent) {
        (true, Ok(()), false) => Ok(()),
        (true, Ok(()), true) => Err(stale_flag_msg.into()),
        (true, Err(e), true) => {
            eprintln!("PARDONED: {} -> {e}", fixture.path.display());
            Ok(())
        }
        (true, Err(e), false) => Err(format!("expected decode/round-trip to succeed, but failed: {e}")),
        (false, Err(_), false) => Ok(()),
        (false, Err(_), true) => Err(stale_flag_msg.into()),
        (false, Ok(()), true) => Ok(()),
        (false, Ok(()), false) => Err(format!(
            "expected decode to fail (well_formed=false){}, but it succeeded",
            fixture.expectations.description.as_deref().map(|d| format!(" — {d}")).unwrap_or_default()
        )),
    }
}

/// Decode the block once from the fixture bytes,
/// then encode it, and decode it again from the encoding
///
/// Check that we get the same values after decoding
fn check_block(bytes: &[u8]) -> Result<(), cbor::decode::Error> {
    let (era, block): (EraName, Block) = from_cbor_no_leftovers(bytes)?;
    let re_encoded = to_cbor(&(era, block.clone()));
    let (re_era, re_block): (EraName, Block) = from_cbor_no_leftovers(&re_encoded)?;
    if era != re_era {
        return Err(cbor::decode::Error::message(format!("era changed across round-trip: {era:?} → {re_era:?}")));
    }
    if !block.cbor_eq(&re_block) {
        return Err(cbor::decode::Error::message("block CBOR round-trip produced different encoded bytes"));
    }
    Ok(())
}

/// Render a category's non-conformant fixtures as a small indented list:
/// `<kind>/<short-hash>  <description>`. The hash is truncated to 8 chars
/// + an ellipsis so the line stays readable.
fn print_divergence_list(list: &[&Fixture], title: &str) {
    if list.is_empty() {
        return;
    }
    eprintln!("  {} ({}):", title, list.len());
    for fx in list {
        let short = short_fixture_path(&fx.path);
        let desc = fx.expectations.description.as_deref().unwrap_or("(no description)");
        eprintln!("    - {short}  {desc}");
    }
}

/// Strip the absolute prefix up to `cbor.decode/`, then shorten the trailing
/// hash directory component to 8 characters + an ellipsis.
fn short_fixture_path(path: &Path) -> String {
    let display = path.display().to_string();
    let relative = display.rsplit("cbor.decode/").next().unwrap_or(&display);
    match relative.rsplit_once('/') {
        Some((parent, last)) if last.len() > 12 => format!("{parent}/{}…", &last[..8]),
        _ => relative.to_string(),
    }
}

/// Decode the transaction body once from the fixture bytes,
/// then encode it, and decode it again from the encoding
///
/// Check that we get the same values after decoding
fn check_transaction_body(bytes: &[u8]) -> Result<(), cbor::decode::Error> {
    let body: TransactionBody = from_cbor_no_leftovers(bytes)?;
    let re_encoded = to_cbor(&body);
    let re_body: TransactionBody = from_cbor_no_leftovers(&re_encoded)?;
    assert_eq!(body, re_body);
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Expectations {
    well_formed: bool,
    #[serde(default)]
    description: Option<String>,
    /// Origin of the fixture. `"cuddle"` for cuddle-generated positives,
    /// `"antigen"` for zapped negatives, absent for committed on-chain samples.
    #[serde(default)]
    source: Option<String>,
    /// Set to `true` when amaru's behavior on this fixture is known to diverge from
    /// the expected (CDDL-canonical) behavior. Acknowledges any of:
    ///   - amaru rejects a CDDL-valid positive (`well_formed: true` + decode fails),
    ///   - amaru accepts a CDDL-invalid negative (`well_formed: false` + decode succeeds),
    ///   - amaru's encoder doesn't reproduce the input bytes on round-trip.
    ///
    /// These fixtures are kept (not pruned) so the divergence stays visible and
    /// trackable. Remove the flag once amaru is fixed.
    #[serde(default)]
    known_amaru_divergence: bool,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Block,
    TransactionBody,
}

impl Kind {
    fn from_root(name: &str) -> Option<Self> {
        match name {
            "block" => Some(Kind::Block),
            "transaction_body" => Some(Kind::TransactionBody),
            _ => None,
        }
    }
}

struct Fixture {
    path: PathBuf,
    kind: Kind,
    bytes: Vec<u8>,
    expectations: Expectations,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/cbor.decode")
}
