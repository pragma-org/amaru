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
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use amaru_kernel::{
    ProtocolVersion,
    protocol_version::{PROTOCOL_VERSION_10, PROTOCOL_VERSION_11},
};
use amaru_uplc::{arena::Arena, flat, syn::parse_program};
const FAILURE_MARKERS: [&str; 3] = ["parse error", "evaluation failure", "out of budget"];

/// Generate flat-encoded files from textual UPLC representations for the conformance tests.
#[expect(clippy::unwrap_used)]
fn main() -> Result<(), Box<dyn Error>> {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/conformance"));
    let mut generated = 0;
    let mut skipped = 0;

    visit(&root, &mut |path| {
        if path.extension().is_none_or(|extension| extension != "uplc") {
            return Ok(());
        }

        let protocol_version = protocol_version(path)?;
        let source = fs::read_to_string(path)?;
        let source_flat = path.with_extension("flat");
        if !source_flat.exists() {
            fs::write(&source_flat, encode_program(&source, protocol_version)?.unwrap_or_default())?;
            generated += 1;
        } else {
            skipped += 1;
        }

        let expected = path.with_file_name(format!("{}.expected", path.file_name().unwrap().to_string_lossy()));
        let expected_flat = path.with_extension("flat.expected");
        if !expected_flat.exists() {
            let expected = fs::read_to_string(expected)?;
            let value = if let Some(marker) = FAILURE_MARKERS.iter().find(|marker| expected.trim() == **marker) {
                marker.to_string()
            } else {
                encode_program(&expected, protocol_version)?.unwrap_or_default()
            };
            fs::write(expected_flat, value)?;
            generated += 1;
        } else {
            skipped += 1;
        }
        Ok(())
    })?;

    eprintln!("generated {generated} Flat artifacts; skipped {skipped} existing artifacts");
    Ok(())
}

fn visit(path: &Path, callback: &mut impl FnMut(&Path) -> Result<(), Box<dyn Error>>) -> Result<(), Box<dyn Error>> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            visit(&entry?.path(), callback)?;
        }
    } else {
        callback(path)?;
    }
    Ok(())
}

fn protocol_version(path: &Path) -> Result<ProtocolVersion, Box<dyn Error>> {
    for component in path.components() {
        match component.as_os_str().to_string_lossy().as_ref() {
            "protocol-version-10" => return Ok(PROTOCOL_VERSION_10),
            "protocol-version-11" => return Ok(PROTOCOL_VERSION_11),
            _ => {}
        }
    }
    Err(format!("{} is not below a protocol-version directory", path.display()).into())
}

fn encode_program(source: &str, protocol_version: ProtocolVersion) -> Result<Option<String>, Box<dyn Error>> {
    let arena = Arena::new();
    let Some(program) = parse_program(&arena, source, protocol_version).into_result().ok() else {
        return Ok(None);
    };
    Ok(flat::encode(program).ok().map(hex::encode))
}
