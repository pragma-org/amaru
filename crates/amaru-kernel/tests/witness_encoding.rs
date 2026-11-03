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

use std::path::{Component, Path, PathBuf};

use amaru_kernel::{ProtocolVersion, WitnessSet, cbor};
use serde::Deserialize;

const CORPUS_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/witness_encoding");
const MANIFEST: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/witness_encoding/manifest.json"));

#[derive(Debug, Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    protocol_version: String,
    class: String,
    original_size: usize,
    compact_size: usize,
    delta: isize,
    original_path: PathBuf,
    compact_path: PathBuf,
}

#[test]
fn witness_encoding_corpus_matches_the_protocol_aware_encoder() {
    let manifest: Manifest = serde_json::from_str(MANIFEST).expect("valid witness encoding manifest");
    let root = Path::new(CORPUS_ROOT);

    assert!(!manifest.cases.is_empty(), "witness encoding corpus is empty");

    for case in manifest.cases {
        let original_path = corpus_path(root, &case.original_path);
        let compact_path = corpus_path(root, &case.compact_path);

        let original = std::fs::read(&original_path).expect("read original witness set");
        let compact = std::fs::read(&compact_path).expect("read compact witness set");

        assert_eq!(original.len(), case.original_size, "original size mismatch: {}", case.original_path.display());
        assert_eq!(compact.len(), case.compact_size, "compact size mismatch: {}", case.compact_path.display());
        assert_eq!(compact.len() as isize - original.len() as isize, case.delta, "delta mismatch");
        assert!(compact.len() <= original.len(), "compact encoding grew: {}", case.compact_path.display());
        assert_eq!(case.class, if case.delta == 0 { "same" } else { "shrinking" });

        let protocol_version: ProtocolVersion = case.protocol_version.parse().expect("valid protocol version");
        let mut decode_context = protocol_version;
        let witnesses: WitnessSet =
            cbor::from_cbor_no_leftovers_with(&original, &mut decode_context).expect("decode original witness set");

        let mut encode_context = protocol_version;
        let encoded = cbor::to_cbor_with(&witnesses, &mut encode_context);
        assert_eq!(encoded, compact, "compact encoding mismatch: {}", case.compact_path.display());

        let mut compact_decode_context = protocol_version;
        cbor::from_cbor_no_leftovers_with::<_, WitnessSet>(&compact, &mut compact_decode_context)
            .expect("decode compact witness set");
    }
}

fn corpus_path(root: &Path, relative: &Path) -> PathBuf {
    assert!(relative.is_relative(), "corpus path must be relative: {}", relative.display());
    assert!(
        relative.components().all(|component| !matches!(component, Component::ParentDir)),
        "corpus path escapes corpus: {}",
        relative.display()
    );
    root.join(relative)
}
