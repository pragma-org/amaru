# AGENTS.md for Amaru

This file provides instructions for agentic coding tools (e.g. opencode, Cursor agents, etc.) working on the Amaru codebase.

## Build, Lint, and Test Commands

### Core Cargo Aliases (from .cargo/config.toml)
- `cargo check-amaru`: `check --workspace --all-targets`
- `cargo clippy-amaru`: `clippy --workspace --all-targets -- -D warnings`
- `cargo fmt-amaru`: `fmt --all -- --check`
- Use nightly toolchain: `nightly-2025-11-21` (see rust-toolchain.toml)

### Makefile Targets (preferred for consistency)
- `make build`: Build in release profile
- `make start`: Build and run with defaults (uses AMARU_NETWORK=preprod)
- `make all-ci-checks`: Runs fmt, clippy, tests, examples, coverage
- `make test-e2e`: End-to-end snapshot tests (`cargo test -p amaru -- --ignored`)
- `make coverage-html`: Generate HTML coverage report (requires cargo-llvm-cov)
- `make coverage-lconv`: LCOV for Codecov

### Testing Commands
- Run all tests: `cargo test --workspace --all-targets`
- Run tests for specific crate: `cargo test -p amaru-consensus`
- Run single test function: `cargo test test_intersect_found --lib -p amaru-consensus`
- Run specific test file: `cargo test --test tests -p amaru-consensus`
- Run ignored tests: `cargo test -- --ignored`
- Property tests: Use proptest (enabled via test-utils feature)
- Simulation tests: See simulation/amaru-sim (cargo test -p amaru-sim)
- Ledger conformance: `make update-ledger-conformance-test-snapshot` then test
- Bench: `cargo bench --bench stage_msgs --features="test-utils"`
- Single bench: `cargo bench --bench stage_msgs --features test-utils <bench_name>`

### Other
- Format: `cargo fmt-amaru` or `make` targets
- Pre-commit hooks: `./scripts/setup-hooks.sh` (runs clippy)
- Full CI: See .github/workflows/continuous-integration.yml
- For benches requiring features: `--features test-utils`
- Coverage: `cargo llvm-cov --workspace --lcov`

Run `make help` for all targets.

## Code Style Guidelines

### General
- Follow Rust conventions + project-specific rules from EDRs in engineering-decision-records/
- Always check neighboring files, similar modules, and Cargo.toml for existing patterns, libs, imports, error types before adding new code
- Mimic code style, naming, formatting exactly
- Use existing libraries/utilities; NEVER assume a lib is available without checking Cargo.toml and neighbors
- When creating new component: look at existing ones for framework, naming, typing
- When editing: look at surrounding context (imports) to match choices of libs/frameworks
- Follow security best practices: never expose/log secrets/keys, never commit them
- NO comments in code unless explicitly asked (use descriptive names instead)
- All code must compile and pass clippy/fmt
- main branch must always be working (compiles + tests pass)

### Formatting (rustfmt.toml)
- unstable_features = true
- imports_granularity = "Crate"
- group_imports = "StdExternalCrate"
- max_width = 120
- use_small_heuristics = "Max"
- Imports order: std, external crates, workspace/project crates, self/super/crate

### Clippy and Lints (.cargo/clippy.toml)
- allow-expect-in-tests, allow-panic-in-tests, allow-unwrap-in-tests = true
- type-complexity-threshold = 600
- enum-variant-size-threshold = 1024
- Disallow: std::collections::HashMap, HashSet (use BTree* or indexmap if needed)
- Disallow certain unchecked EraHistory methods
- Fix all warnings; CI enforces -D warnings

### Error Handling (EDR 013-error-handling-strategies.md)
- Use Result<T, E> everywhere possible
- Avoid panic! except for bugs/unreachable
- Libs: thiserror::Error enums per crate, implement std::error::Error
- Use #[from] for foreign errors only; avoid for internal
- Bins/apps: anyhow::Result for context with .context()
- For arbitrary errors: Box<dyn std::error::Error + Send + Sync>
- Use ? operator extensively
- See example in EDR for patterns in lib.rs vs main.rs
- Custom errors in crates/amaru-consensus/src/errors.rs etc.

### Types and Naming (EDR 009-avoid-primitive-obsession.md)
- Avoid primitive obsession: use newtypes for domain concepts (Slot, Point, Tip, Peer, Hash, etc.)
- Newtypes: #[derive(...)] #[repr(transparent)] pub struct Slot(u64); with private field
- Implement: Debug, Clone, Copy (if applicable), PartialEq/Eq/Ord, Display, Serialize/Deserialize, CBOR Encode/Decode
- Validation in constructors or From impls
- Naming: snake_case for vars/fns, UpperCamel for types, descriptive
- Use amaru_kernel types where possible (BlockHeader, Tip, Point, EraName, etc.)
- No HashMap/HashSet in public APIs (use BTreeMap for determinism?)

### Imports and Modules
- Group as per rustfmt: std > external > crate
- Use self, super, crate:: as appropriate
- Prefer specific imports over glob (*)
- For effects/stages: follow pure_stage patterns
- See crates/amaru-consensus/src/stages/ for examples

### Headers and Licensing
- All .rs files start with:
  ```rust
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
  ```
- Use Apache 2.0 license

### Git and Commits (from CONTRIBUTING.md)
- Conventional commits
- GPG-signed commits
- Sign-off with -s (--sign-off)
- Small, focused commits; no "wip", "tmp"
- Update CHANGELOG.md for user-facing changes
- Never force push to main
- See EDRs for design docs

### Testing and Simulation
- Unit + property (proptest) + conformance tests
- Deterministic simulation with pure-stage for consensus/network (see EDR 011)
- Prefer simulation tests for complex logic
- Tests in tests/ or mod tests
- Use test_setup.rs patterns in stages
- For single test in sim: cargo test -p amaru-sim <name>

### Project Structure
- crates/: amaru-kernel, amaru-consensus, amaru-ledger, amaru-protocols, amaru-ouroboros, etc.
- simulation/amaru-sim: for DST
- engineering-decision-records/: all major decisions (read before big changes)
- docs/, monitoring/, conformance-tests/
- Use pure_stage for effectful code to enable simulation

### AI/Agent Specific
- Be proactive but only when asked; ask for clarification via question tool if ambiguous
- Use tools extensively: glob/grep/read before editing
- For edits: always Read first, then edit with exact string match
- Verify with lint/test after changes
- Do not create docs unless asked
- When in doubt, mimic closest existing code (use grep/glob)
- Report feedback to https://github.com/anomalyco/opencode/issues

### Useful Commands Summary
- Single test in crate: `cargo test <fn_name> -p <crate_name>`
- Test with filter: `cargo test <filter>`
- Bench specific: `cargo bench --bench <name> <filter> --features test-utils`
- Run simulation test: `cargo test -p amaru-sim`
- Update snapshots: make targets

Follow all EDRs. Read relevant ones before implementing features (e.g. observability, time, errors).

(Generated for agentic use; ~150 lines including whitespace/comments.)
