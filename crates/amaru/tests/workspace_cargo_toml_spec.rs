// These tests validate critical properties of the workspace Cargo.toml,
// focusing on keys and configurations relevant to the provided diff context.
//
// Testing framework: Rust built-in test harness (#[test]).
// We avoid adding new dependencies; tests use std only.
// Conventions: descriptive test names, organized assertions, helpful error messages.
//
// Strategy:
// - Discover workspace root by traversing ancestors until we find a Cargo.toml
//   that contains a [workspace] section (so tests are stable regardless of crate path).
// - Read Cargo.toml as text and assert the presence of critical lines/sections.
// - Prefer exact string checks for stability; avoid TOML parsing to keep dependency-free.
// - Cover happy paths and critical failure conditions (missing keys/sections).

use std::fs;
use std::path::{Path, PathBuf};

fn find_workspace_root(mut start: PathBuf) -> PathBuf {
    // Traverse up until we find Cargo.toml that contains a [workspace] section.
    loop {
        let candidate = start.join("Cargo.toml");
        if candidate.exists() {
            if let Ok(contents) = fs::read_to_string(&candidate) {
                if contents.contains("[workspace]") {
                    return start;
                }
            }
        }
        if !start.pop() {
            panic!("Failed to locate workspace root containing [workspace]");
        }
    }
}

fn read_workspace_cargo_toml() -> (PathBuf, String) {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = find_workspace_root(crate_dir);
    let cargo_toml = root.join("Cargo.toml");
    let data = fs::read_to_string(&cargo_toml)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", cargo_toml.display()));
    (cargo_toml, data)
}

#[test]
fn workspace_package_metadata_is_present_and_correct() {
    let (path, data) = read_workspace_cargo_toml();

    // Required [workspace.package] section
    assert!(
        data.contains("[workspace.package]"),
        "Expected [workspace.package] section in {}",
        path.display()
    );

    // Validate critical keys/values based on the provided snippet/diff focus
    let required_pairs = [
        r#"version = "0.1.0""#,
        r#"edition = "2021""#,
        r#"description = "A Cardano blockchain node implementation""#,
        r#"license = "Apache-2.0""#,
        r#"authors = ["Amaru Maintainers <amaru@pragma.builders>"]"#,
        r#"repository = "https://github.com/pragma-org/amaru""#,
        r#"homepage = "https://github.com/pragma-org/amaru""#,
        r#"documentation = "https://docs.rs/amaru""#,
        r#"rust-version = "1.88""#,
    ];

    for kv in required_pairs {
        assert!(
            data.contains(kv),
            "Missing or changed workspace.package entry: {kv}"
        );
    }
}

#[test]
fn workspace_members_and_resolver_are_configured() {
    let (_path, data) = read_workspace_cargo_toml();

    // [workspace] core settings
    assert!(data.contains("[workspace]"), "Missing [workspace] section");
    assert!(
        data.contains(r#"members = ["crates/*", "simulation/*"]"#),
        "Expected workspace members to include crates/* and simulation/*"
    );
    assert!(
        data.contains(r#"default-members = ["crates/*"]"#),
        "Expected default-members to be crates/*"
    );
    assert!(
        data.contains(r#"resolver = "2""#),
        "Expected resolver = \"2\""
    );
}

#[test]
fn workspace_dependencies_key_flags_and_features_are_present() {
    let (_path, data) = read_workspace_cargo_toml();

    // Validate presence of the [workspace.dependencies] section
    assert!(
        data.contains("[workspace.dependencies]"),
        "Missing [workspace.dependencies] section"
    );

    // Spot check critical dependencies and their flags/features to catch regressions
    let must_contain = [
        // Pallas family
        r#"pallas-addresses = "0.33.0""#,
        r#"pallas-codec = "0.33.0""#,
        r#"pallas-crypto = "0.33.0""#,
        r#"pallas-math = "0.33.0""#,
        r#"pallas-network = "0.33.0""#,
        r#"pallas-primitives = "0.33.0""#,
        r#"pallas-traverse = "0.33.0""#,

        // Minicbor version alignment
        r#"minicbor = "0.25.1""#,

        // Gasket pin + derive feature
        r#"gasket = { version = "0.8.0", features = ["derive"] }"#,

        // Serde default-features disabled
        r#"serde = { version = "1.0", default-features = false }"#,

        // Reqwest with json, stream, rustls-tls features and default-features disabled
        r#"reqwest = { version = "0.12.20", default-features = false, features = ["json", "stream", "rustls-tls"] }"#,

        // Tokio sync feature
        r#"tokio = { version = "1.45.0", features = ["sync"] }"#,

        // RocksDB binding features
        r#""snappy","#,
        r#""multi-threaded-cf""#,
    ];

    for needle in must_contain {
        assert!(
            data.contains(needle),
            "Expected workspace dependency/config to include: {needle}"
        );
    }

    // Ensure rocksdb stanza mentions bindgen-runtime and default-features = false
    assert!(
        data.contains(r#"rocksdb = { version = "0.23.0", default-features = false, features = ["#),
        "Expected rocksdb dependency with default-features = false and features list"
    );
    assert!(
        data.contains(r#""bindgen-runtime""#),
        "Expected rocksdb feature 'bindgen-runtime'"
    );
}

#[test]
fn git_pinned_crypto_dependencies_are_locked_by_rev() {
    let (_path, data) = read_workspace_cargo_toml();

    // Validate VRF and KES are pinned by exact 'rev' commits
    assert!(
        data.contains(r#"vrf_dalek = { git = "https://github.com/txpipe/vrf", rev = "044b45a1a919ba9d9c2471fc5c4d441f13086676" }"#),
        "Expected vrf_dalek to be locked to specific git rev"
    );
    assert!(
        data.contains(r#"kes-summed-ed25519 = { git = "https://github.com/txpipe/kes", rev = "f69fb357d46f6a18925543d785850059569d7e78" }"#),
        "Expected kes-summed-ed25519 to be locked to specific git rev"
    );
}

#[test]
fn dev_dependencies_include_testing_utilities() {
    let (_path, data) = read_workspace_cargo_toml();

    // Ensure dev-dependencies exist and include critical testing tools
    let expected = [
        r#"criterion = "0.6.0""#,
        r#"ctor = "0.4.1""#,
        r#"insta = "1.41.1""#,
        r#"once_cell = "1.21.3""#,
        r#"pretty_assertions = "1.4.1""#,
        r#"proptest = { version = "1.5.0", default-features = false, features = ["std"] }"#,
        r#"rand = "0.9.1""#,
        r#"rand_distr = "0.5.1""#,
        r#"tempfile = "3.20.0""#,
        r#"test-case = "3.3.1""#,
    ];

    for dep in expected {
        assert!(
            data.contains(dep),
            "Missing dev-dependency: {dep}"
        );
    }
}

#[test]
fn lints_and_profiles_are_enforced() {
    let (_path, data) = read_workspace_cargo_toml();

    // Lints
    assert!(
        data.contains("[workspace.lints.rust]"),
        "Missing [workspace.lints.rust] section"
    );
    let lint_expectations = [
        r#"rust-2018-idioms = "warn""#,
        r#"rust-2018-compatibility = "warn""#,
        r#"rust-2021-compatibility = "warn""#,
        r#"nonstandard-style = { level = "deny" }"#,
        r#"future-incompatible = { level = "deny" }"#,
    ];
    for l in lint_expectations {
        assert!(data.contains(l), "Missing or changed rust lint: {l}");
    }

    assert!(
        data.contains("[workspace.lints.clippy]"),
        "Missing [workspace.lints.clippy] section"
    );
    let clippy_expectations = [
        r#"expect_used = "warn""#,
        r#"panic = "warn""#,
        r#"todo = "warn""#,
        r#"unwrap_used = "warn""#,
        r#"wildcard_enum_match_arm = "warn""#,
    ];
    for l in clippy_expectations {
        assert!(data.contains(l), "Missing or changed clippy lint: {l}");
    }

    // Profiles
    assert!(data.contains("[profile.dev]"), "Missing [profile.dev]");
    assert!(data.contains(r#"opt-level = 2"#), "Expected profile.dev opt-level = 2");
    assert!(data.contains(r#"debug = false"#), "Expected profile.dev debug = false");

    assert!(data.contains("[profile.release]"), "Missing [profile.release]");
    assert!(data.contains(r#"lto = true"#), "Expected profile.release lto = true");
    assert!(data.contains(r#"codegen-units = 1"#), "Expected profile.release codegen-units = 1");
}

#[test]
fn cross_compilation_metadata_is_defined_for_musl_target() {
    let (_path, data) = read_workspace_cargo_toml();

    // Cross metadata presence and key commands/env
    assert!(
        data.contains("[workspace.metadata.cross.target.aarch64-unknown-linux-musl]"),
        "Missing cross target metadata for aarch64-unknown-linux-musl"
    );

    assert!(
        data.contains(r#"pre-build = ["#) && data.contains(r#""apt update""#) && data.contains(r#""apt install -y libclang-dev""#),
        "Expected pre-build steps to include apt update and libclang-dev installation"
    );

    assert!(
        data.contains("[workspace.metadata.cross.target.aarch64-unknown-linux-musl.env]"),
        "Missing cross target env metadata"
    );
    assert!(
        data.contains(r#"passthrough = ["LIBCLANG_PATH=/usr/lib/llvm-10/lib"]"#),
        "Expected LIBCLANG_PATH passthrough"
    );
}

// Negative/edge test: ensure we don't accidentally have root [package] overriding workspace.package
#[test]
fn no_root_package_section_conflicts_with_workspace() {
    let (path, data) = read_workspace_cargo_toml();
    assert!(
        !data.contains("\n[package]"),
        "Workspace root Cargo.toml at {} should not define a root [package]; it should use [workspace.package]",
        path.display()
    );
}