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

//! Compile-fail snapshots for session typestate diagnostics.
//!
//! Snippets in `tests/ui/*.rs` are type-checked with `rustc` against the
//! `amaru_pure_stage` rlib already built for this test binary (no second Cargo
//! graph). Expected diagnostics live in the adjacent `*.stderr` file.
//!
//! Refresh snapshots with `BLESS=1 cargo test -p amaru-pure-stage --test ui`.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use serde_json::Value;

#[test]
fn session_typestate_diagnostics() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ui_dir = manifest.join("tests/ui");
    let lib_dirs = collect_lib_dirs();
    let externs = crate_externs(&lib_dirs, &direct_crate_names(&manifest));
    assert!(
        externs.iter().any(|(name, _)| name == "amaru_pure_stage"),
        "could not find libamaru_pure_stage (searched {}; started at {})",
        lib_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
        env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
    );
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let target = rustc_target();
    let out_dir = env::temp_dir().join("amaru-pure-stage-ui");
    fs::create_dir_all(&out_dir).unwrap();

    let mut cases: Vec<PathBuf> = fs::read_dir(&ui_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "rs") && p.file_stem().is_some_and(|s| s != "harness"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no tests/ui/*.rs snippets");

    let bless = env::var_os("BLESS").is_some() || env::var_os("AMARU_UI_BLESS").is_some();
    let mut failures = Vec::new();

    for src in &cases {
        let rel = src.strip_prefix(&manifest).unwrap_or(src);
        let expected_path = src.with_extension("stderr");
        let actual = compile_snippet(&rustc, &manifest, rel, &lib_dirs, &externs, target.as_deref(), &out_dir);
        if bless {
            fs::write(&expected_path, &actual).unwrap();
            continue;
        }
        let expected = fs::read_to_string(&expected_path).unwrap_or_default();
        if expected != actual {
            failures.push(format!("{}:\n{}", rel.display(), pretty_assertions::Comparison::new(&expected, &actual)));
        }
    }

    assert!(
        failures.is_empty(),
        "session UI diagnostics mismatch (BLESS=1 to update snapshots):\n\n{}",
        failures.join("\n\n")
    );
}

fn compile_snippet(
    rustc: &str,
    manifest: &Path,
    rel: &Path,
    lib_dirs: &[PathBuf],
    externs: &[(String, PathBuf)],
    target: Option<&str>,
    out_dir: &Path,
) -> String {
    let mut cmd = Command::new(rustc);
    cmd.current_dir(manifest).args([
        "--edition=2024",
        "--crate-type=lib",
        "--emit=metadata",
        "--error-format=json",
        "--diagnostic-width=200",
        "--cap-lints=allow",
        "-A=unused",
        "-A=dead_code",
    ]);
    if let Some(target) = target {
        cmd.arg("--target").arg(target);
    }
    cmd.arg("--out-dir").arg(out_dir);
    for dir in lib_dirs {
        cmd.arg("-L").arg(format!("dependency={}", dir.display()));
    }
    for (name, path) in externs {
        cmd.arg("--extern").arg(format!("{name}={}", path.display()));
    }
    let output = cmd.arg(rel).output().unwrap_or_else(|e| panic!("failed to spawn {rustc}: {e}"));

    if output.status.success() {
        panic!("{} compiled; expected a session diagnostic", rel.display());
    }

    format_stderr(&String::from_utf8_lossy(&output.stderr), manifest)
}

fn format_stderr(stderr: &str, manifest: &Path) -> String {
    let mut out = String::new();
    for line in stderr.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(diag) = diagnostic_object(&v) else {
            continue;
        };
        if diag.get("level").and_then(Value::as_str) != Some("error") {
            continue;
        }
        write_diag(&mut out, diag, manifest, 0);
    }
    out
}

fn diagnostic_object(v: &Value) -> Option<&Value> {
    if v.get("message").is_some() && v.get("level").is_some() {
        return Some(v);
    }
    v.get("message").filter(|m| m.get("level").is_some())
}

fn write_diag(out: &mut String, diag: &Value, manifest: &Path, indent: usize) {
    let pad = "  ".repeat(indent);
    let level = diag.get("level").and_then(Value::as_str).unwrap_or("error");
    let msg = diag.get("message").and_then(Value::as_str).unwrap_or("");
    if skip_note(msg) {
        return;
    }
    let code = diag.get("code").and_then(|c| c.get("code")).and_then(Value::as_str);
    match code {
        Some(code) if indent == 0 => out.push_str(&format!("{pad}{level}[{code}]: {msg}\n")),
        _ if indent == 0 => out.push_str(&format!("{pad}{level}: {msg}\n")),
        Some(code) => out.push_str(&format!("{pad}= {level}[{code}]: {msg}\n")),
        None => out.push_str(&format!("{pad}= {level}: {msg}\n")),
    }

    if let Some(span) = diag
        .get("spans")
        .and_then(Value::as_array)
        .and_then(|s| s.iter().find(|s| s.get("is_primary").and_then(Value::as_bool) == Some(true)))
    {
        let file = span.get("file_name").and_then(Value::as_str).unwrap_or("");
        let line = span.get("line_start").and_then(Value::as_u64).unwrap_or(0);
        let col = span.get("column_start").and_then(Value::as_u64).unwrap_or(0);
        let file = normalize_path(file, manifest);
        if indent == 0 || file.starts_with("tests/") {
            out.push_str(&format!("{pad} --> {file}:{line}:{col}\n"));
        }
        if let Some(label) = span.get("label").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            out.push_str(&format!("{pad}  = {label}\n"));
        }
    }

    if let Some(children) = diag.get("children").and_then(Value::as_array) {
        for child in children {
            write_diag(out, child, manifest, indent + 1);
        }
    }
}

fn skip_note(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("full type name has been written")
        || m.contains("full name for the type has been written")
        || m.contains("consider using `--verbose`")
        || m.contains("for more information about this error")
        || m.contains("long-type-")
        || m.contains("aborting due to")
}

fn normalize_path(file: &str, manifest: &Path) -> String {
    let path = Path::new(file);
    if let Ok(rel) = path.strip_prefix(manifest) {
        return rel.display().to_string().replace('\\', "/");
    }
    file.replace('\\', "/")
}

fn collect_lib_dirs() -> Vec<PathBuf> {
    let profile = find_profile_dir();
    let mut dirs = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() && seen.insert(p.clone()) {
            dirs.push(p);
        }
    };
    add_profile_libs(&profile, &mut push);
    // `--target` puts the test binary under `target/<triple>/debug`, but proc
    // macros stay on the host in `target/debug`.
    if let Some(triple_dir) = profile.parent()
        && triple_dir.file_name().and_then(|n| n.to_str()).is_some_and(|s| s.contains('-'))
        && let Some(target_root) = triple_dir.parent()
    {
        add_profile_libs(&target_root.join("debug"), &mut push);
        add_profile_libs(&target_root.join("release"), &mut push);
    }
    dirs
}

fn add_profile_libs(profile: &Path, push: &mut impl FnMut(PathBuf)) {
    push(profile.join("deps"));
    let Ok(crates) = fs::read_dir(profile.join("build")) else {
        return;
    };
    for crate_dir in crates.flatten() {
        let Ok(hashes) = fs::read_dir(crate_dir.path()) else {
            continue;
        };
        for hash in hashes.flatten() {
            let out = hash.path().join("out");
            if rustc_lib_dir(&out) {
                push(out);
            }
        }
    }
}

fn find_profile_dir() -> PathBuf {
    let exe = env::current_exe().expect("current_exe");
    let mut dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    for _ in 0..12 {
        if dir.join("deps").is_dir() || has_build_outs(&dir.join("build")) {
            return dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    panic!("could not find cargo profile dir (started at {})", exe.display());
}

fn rustc_lib_dir(dir: &Path) -> bool {
    let Ok(ents) = fs::read_dir(dir) else {
        return false;
    };
    ents.flatten().any(|ent| {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        name.starts_with("lib")
            && (name.ends_with(".rlib")
                || name.ends_with(".rmeta")
                || name.ends_with(".so")
                || name.ends_with(".dylib")
                || name.ends_with(".dll"))
    })
}

fn has_build_outs(build: &Path) -> bool {
    let Ok(crates) = fs::read_dir(build) else {
        return false;
    };
    for crate_dir in crates.flatten() {
        let Ok(hashes) = fs::read_dir(crate_dir.path()) else {
            continue;
        };
        if hashes.flatten().any(|hash| hash.path().join("out").is_dir()) {
            return true;
        }
    }
    false
}

fn rustc_target() -> Option<String> {
    if let Ok(t) = env::var("CARGO_BUILD_TARGET")
        && !t.is_empty()
    {
        return Some(t);
    }
    let exe = env::current_exe().ok()?;
    for dir in exe.ancestors() {
        let name = dir.file_name()?.to_str()?;
        if name != "debug" && name != "release" {
            continue;
        }
        let triple = dir.parent()?.file_name()?.to_str()?;
        if triple.contains('-') && triple != "target" {
            return Some(triple.to_string());
        }
    }
    None
}

fn direct_crate_names(manifest_dir: &Path) -> Vec<String> {
    let mut names = vec!["amaru_pure_stage".into()];
    let Ok(toml) = fs::read_to_string(manifest_dir.join("Cargo.toml")) else {
        return names;
    };
    let mut in_deps = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]" || (t.starts_with("[target.") && t.contains("dependencies"));
            continue;
        }
        if !in_deps || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let name = t.split([' ', '.', '=', '{']).next().unwrap_or("");
        if !name.is_empty() {
            names.push(name.replace('-', "_"));
        }
    }
    names
}

/// One rlib + matching rmeta per crate. Hashes are chosen so they appear in
/// `amaru_pure_stage`'s rmeta (same pairing cargo passes as `--extern`).
fn crate_externs(dirs: &[PathBuf], crate_names: &[String]) -> Vec<(String, PathBuf)> {
    let Some(stage) = crate_artifact(dirs, "amaru_pure_stage", &["rlib"]) else {
        return Vec::new();
    };
    let stage_rmeta = stage.with_extension("rmeta");
    let meta = fs::read(&stage_rmeta).unwrap_or_default();
    let mut out = vec![("amaru_pure_stage".into(), stage)];
    if fs::metadata(&stage_rmeta).is_ok_and(|m| m.len() > 0) {
        out.push(("amaru_pure_stage".into(), stage_rmeta));
    }
    for name in crate_names {
        if name == "amaru_pure_stage" {
            continue;
        }
        let mut cands: Vec<(SystemTime, PathBuf)> = Vec::new();
        for dir in dirs {
            if let Some(path) = newest_hashed(dir, name, &["rlib"])
                && let Ok(mtime) = fs::metadata(&path).and_then(|m| m.modified())
            {
                cands.push((mtime, path));
            }
        }
        cands.sort_by_key(|a| std::cmp::Reverse(a.0));
        let chosen = cands
            .iter()
            .find(|(_, p)| extra_filename(p).is_some_and(|h| contains_bytes(&meta, h.as_bytes())))
            .or(cands.first());
        let Some((_, rlib)) = chosen else {
            continue;
        };
        out.push((name.clone(), rlib.clone()));
        let rmeta = rlib.with_extension("rmeta");
        if fs::metadata(&rmeta).is_ok_and(|m| m.len() > 0) {
            out.push((name.clone(), rmeta));
        }
    }
    out
}

fn extra_filename(path: &Path) -> Option<&str> {
    path.file_stem()?.to_str()?.rsplit_once('-').map(|(_, hash)| hash)
}

fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn crate_artifact(dirs: &[PathBuf], crate_name: &str, exts: &[&str]) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for dir in dirs {
        let Some(path) = newest_hashed(dir, crate_name, exts) else {
            continue;
        };
        let Ok(mtime) = fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| mtime >= *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

fn newest_hashed(dir: &Path, crate_name: &str, exts: &[&str]) -> Option<PathBuf> {
    let prefix = format!("lib{crate_name}-");
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for ent in fs::read_dir(dir).ok()?.flatten() {
        let path = ent.path();
        let name = path.file_name()?.to_string_lossy();
        if !name.starts_with(&prefix) {
            continue;
        }
        if !exts.iter().any(|ext| name.ends_with(&format!(".{ext}"))) {
            continue;
        }
        let meta = fs::metadata(&path).ok()?;
        if meta.len() == 0 {
            continue;
        }
        let mtime = meta.modified().ok()?;
        if best.as_ref().is_none_or(|(t, _)| mtime >= *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}
