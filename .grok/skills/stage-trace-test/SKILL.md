---
name: stage-trace-test
description: Fix failing pure-stage simulation tests that use `assert_trace_match()` by running the test, reading the trace diff in the failure output, and updating the expected trace using `tm_*` helpers from `amaru_pure_stage` (prefer `tm_state_match` for targeted state checks, `tm_send_match` for message properties). Handles missing/broken assertions, deserializer registration (`register_effect_deserializer` / `register_data_deserializer`), and crate-specific trace helpers in `test_utils.rs`. Use when a trace assertion test fails, when updating `assert_trace_match` expectations, when fixing simulation trace mismatches, or when the user runs /stage-trace-test.
---

# Fix `assert_trace_match` Test Failures

Update failing pure-stage simulation tests so `assert_trace_match()` passes again.

## Workflow

### 1. Identify the failing test

The user will name a test function, file, or stage. Locate it under `crates/amaru-consensus/src/stages/<stage>/tests.rs` (or `amaru-pure-stage` tests). Read the stage's `test_setup.rs` for `register_guards()`, `setup()`, and any existing `te_*` / `tm_*` helpers.

### 2. Run the test and capture the diff

```bash
cargo test <test_name> -p amaru-consensus -- --nocapture
```

Or for pure-stage tests:

```bash
cargo test <test_name> -p amaru-pure-stage -- --nocapture
```

On trace mismatch, `pretty_assertions::assert_eq!` prints a diff between **left** (actual trace) and **right** (expected `TraceMatch` list). Read both sides carefully before editing.

### 3. Diagnose the failure type

**A. Trace content mismatch** — actual entries differ from expected. Update the `assert_trace_match` call.

**B. Generic/untyped representation** — actual shows `typetag` with Map/array structure instead of a typed Rust value. A deserializer is missing from `register_guards()` in the stage's `test_setup.rs`:
- External effects → `amaru_pure_stage::register_effect_deserializer::<T>().boxed()`
- Messages, state, tuples, and other `SendData` → `amaru_pure_stage::register_data_deserializer::<T>().boxed()`

Re-run the test after adding guards; the trace should show properly typed values.

**C. No assertion or broken assertion** — test has no `assert_trace_match`, or the expected list is too wrong to patch incrementally. Replace the expected slice with `&[]` temporarily, run the test, and use the full actual trace printed on the left side as the starting point.

### 4. Translate actual trace entries to `TraceMatch` values

Use `tm_*` constructors from `amaru_pure_stage::trace_match` (re-exported via `test_utils.rs`):

| Trace entry kind | Exact match | Property match (preferred when possible) |
|---|---|---|
| State | `tm_state(stage, &state)` | `tm_state_match(stage, \|s\| ...)` |
| Input | `tm_input(stage, &msg)` | — |
| Send | `tm_send(from, to, msg)` | `tm_send_match(from, to, \|m\| ...)`, `tm_send_type::<T>(from, to)` |
| Terminate | `tm_terminate(stage)` | — |
| Terminated | `tm_terminated(stage, reason)` | — |
| AddStage | `tm_add_stage(at, name)` | — |
| WireStage | `tm_wire_stage(parent, child)` | `tm_wire_stage_state`, `tm_wire_stage_state_supervised` |

**Matching strategy:**
- Prefer **property matchers** (`tm_state_match`, `tm_send_match`) when only part of a value matters (e.g. a single field changed, dynamic stage suffixes, random peer IDs).
- Use **exact matchers** (`tm_state`, `tm_send`, `tm_input`) when the full value is stable and intentional.
- For external effects with variable payloads, use or create a property matcher (see `tm_record_metrics` in `validate_block/test_setup.rs` as a pattern).

Existing `te_*` helpers in `test_utils.rs` or per-stage `test_setup.rs` build `TraceEntry` values. Any `TraceEntry` converts to `TraceMatch` via `From`, so `te_load_header(...)` etc. still work inside `assert_trace_match` lists.

### 5. Create new helpers when needed

When a trace entry type has no suitable `tm_*` helper:

- **Generic, reusable** → add to `crates/amaru-pure-stage/src/trace_match.rs`
- **Consensus-specific** (particular effect types, stage naming conventions) → add to `crates/amaru-consensus/src/stages/test_utils.rs` or the stage's `test_setup.rs`

Follow existing patterns:
- `tm_record_metrics` — property match on `Effect::External` with `cast_ref`
- `tm_state_match` / `tm_send_match` in `trace_match.rs` — typed `cast_ref` + predicate + human-readable description string

### 6. Verify

Re-run the failing test until it passes. Then run the stage's full test module to catch regressions:

```bash
cargo test -p amaru-consensus --test tests stages::<stage>::tests
```

Or the relevant test file filter. Ensure `cargo clippy-amaru` is clean if you added new helpers.

## Key files

- `crates/amaru-pure-stage/src/trace_match.rs` — `assert_trace_match`, all `tm_*` helpers
- `crates/amaru-consensus/src/stages/test_utils.rs` — `run_simulation`, `assert_trace`, shared `te_*` / `tm_*`
- `crates/amaru-consensus/src/stages/<stage>/test_setup.rs` — `register_guards()`, stage-specific `te_*` / `tm_*`
- `crates/amaru-consensus/src/stages/<stage>/tests.rs` — test bodies with assertions

## Common pitfalls

- `Resume` entries are filtered out by `assert_trace_match`; do not include them in expected lists.
- Stage names in traces may include random suffixes (e.g. `tp-1/child-abc`). Use `contains` matchers (`tm_send`, `tm_wire_stage`) or property predicates rather than exact names when appropriate.
- `register_guards()` return value must be held for the test lifetime (typically `_guards` or `guards` in `setup()`).
- When migrating from `assert_trace` to `assert_trace_match`, replace `te_*` literals with `tm_*` equivalents only where exact matching is too brittle.