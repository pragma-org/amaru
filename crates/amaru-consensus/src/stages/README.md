# Testing Consensus Stages

This directory contains the consensus stages built on the [`pure-stage`](../../pure-stage) actor library together with their deterministic unit-test harnesses.

Each stage `foo` is implemented in `foo/mod.rs` as an `async fn stage(state: Foo, msg: FooMsg, eff: Effects<FooMsg>) -> Foo`. The pure-stage simulation backend records every effect the stage performs, enabling precise, replayable tests that cover all control-flow paths.

## Per-Stage Test Layout

A stage `foo` always has:

- `foo/mod.rs` — the production stage (and `#[cfg(test)] mod test_setup; mod tests;`)
- `foo/test_setup.rs` — scaffolding, `TestPrep`, harness, and stage-specific trace helpers
- `foo/tests.rs` — the actual test cases

### `test_setup.rs` — Responsibilities

`test_setup.rs` provides everything needed to run the stage under the deterministic simulator and observe its behaviour.

**Core pieces (intended structure):**

- **Test data helpers**
  - Constructors such as `make_block_header(...)`, `TestPrep::peer("...")`, header trees, etc.
  - Constants (`COOLDOWN_SECS`, timeouts, etc.) and small factory functions (`first_schedule_id()`, `cooldown_instant()`, …).

- **`TestPrep` struct**
  ```rust
  pub struct TestPrep {
      pub state: TheStageState,
      pub rt: tokio::runtime::Runtime,   // current-thread runtime required by the simulator
      // plus any external resources (InMemConsensusStore, mocks, …) the stage needs
  }
  ```
  `pub fn test_prep(...) -> TestPrep` builds an initial state in a particular configuration. Multiple variants of `test_prep` are normal when different starting worlds are required.

- **`register_guards() -> DeserializerGuards`**
  Returns a `Vec` of `pure_stage::register_data_deserializer::<T>().boxed()` for every serialisable type that may appear in a trace entry (the stage’s own state and message types, any child stages it wires up, `ManagerMessage`, `ScheduleId`, external-effect request/response types, etc.).

  **Why this is necessary:** When the `TraceBuffer` is deserialized during simulation, the default behaviour yields only a generic JSON-like representation (`cbor4ii::core::Value`) for effects, messages, and states. Registering the concrete types allows the deserializer to recognise the proper Rust types and hydrate the trace entries back into their original, strongly-typed forms. This makes it possible to write readable `assert_trace` expectations that match against the natural Rust values (e.g. `te_state("sc-1", &my_state)`) instead of opaque generic data.

- **`setup` / `setup_preload` harness** (the heart of the scaffolding)
  ```rust
  pub fn setup(prep: &TestPrep, msg: Msg) -> (SimulationRunning, DeserializerGuards, Logs)
  pub fn setup_preload(prep: &TestPrep, msgs: impl IntoIterator<Item = Msg>) -> ...
  ```
  What it does:
  1. Creates a `BufferWriter` and installs a `tracing_subscriber` that writes DEBUG-and-above logs into it.
  2. Builds a `SimulationBuilder` with a shared `TraceBuffer`.
  3. Declares the stage: `let s = network.stage("short-name", super::stage);`
  4. Wires it up with the prepared state from `TestPrep`.
  5. Preloads the input message(s).
  6. Calls `network.run()`.
  7. **Overrides every external effect** the *parent* stage itself performs via `running.override_external_effect::<EffectType>(...)`.
  8. **Enables virtual child stages** by calling `running.use_virtual_child_stages(true)`. This is now the **recommended default** for all stage tests (see dedicated section below). It lets `eff.stage(...)` + `eff.wire_up(...)` calls succeed and be recorded in the trace without materializing real child stages.
  9. Advances the simulation with `run_until_blocked_or_time_incl_effects(...)`.
  10. Returns the `SimulationRunning` (its trace buffer now holds the complete execution), the deserializer guards, and a `Logs` object with captured output.

- **Effect-trace helpers (`te_*`)**
  - One function per external effect or schedule kind used by the stage, e.g. `te_load_header`, `te_validate_block`, `te_schedule`, `te_get_children`, `te_cancel_schedule`, `te_clock`, …
  - These return the appropriate `TraceEntry::Suspend(Effect::...)` or `TraceEntry::Clock(...)` value so that tests can write readable expected traces.
  - Generic helpers (`te_send`, `te_terminate`, `te_terminated`, `te_state`, `te_input`, `assert_trace`) are currently duplicated in many `test_setup.rs` files.

  **Planned move:** the generic `te_*` constructors and `assert_trace` (together with `BufferWriter`/`Logs`) will be moved from the individual `test_setup.rs` files into the shared `../test_utils.rs`. After the refactoring, a stage’s `test_setup.rs` will contain only:
  - its `TestPrep` / `test_prep` / `setup` harness, and
  - the `te_*` functions that are specific to the external effects of *that* stage.

### Virtual Child Stages — The Default Testing Strategy

Many consensus stages dynamically create helper/child stages (e.g. `peer_selection` creates a `"peer-selection/ledger-check"` child via `eff.stage(...)` followed by `eff.wire_up(...)`). In the past this forced test authors either to implement the child fully or to install a large number of `override_external_effect` handlers for every effect the child could ever perform.

**The strongly recommended and now-default approach is to use "virtual child stages":**

```rust
let mut running = network.run();
running.use_virtual_child_stages(true);

// Only override external effects that the *parent* itself performs.
running.override_external_effect::<SomeEffect>(...);

running.run_until_blocked_or_time_incl_effects(...);
```

**What happens in virtual mode**
- The parent's calls to `eff.stage("helper", ...)` and `eff.wire_up(...)` succeed exactly as they would in production.
- The corresponding `Effect::AddStage`, `Effect::WireStage` (plus the matching `StageResponse` resumes) are recorded in the `TraceBuffer`.
- The child's *intended* initial state is still pushed to the trace (`push_state`), so you can assert on it with `te_state` helpers if desired.
- No real `StageData` for the child is inserted into the simulation.
- Any later `Send` (or `Call`) from the parent to the `StageRef` returned by the virtual `wire_up` is treated as delivery to a non-existent stage: the message is dropped and the sender is resumed (visible in the trace as a `Send` effect whose target never produces an `Input`).

**Benefits**
- You can fully test the parent's decision logic, the names and initial states it chooses for children, the messages it sends to them, and how it reacts afterwards — all via clean `assert_trace` expectations.
- No need to write or maintain any code for the child stage's behavior or its external effects.
- The test remains fast and focused on the unit under test (the parent).

**Testing first-message child wiring (recommended pattern)**

Many stages perform lazy child creation on their very first incoming message, for example:

```rust
if self.cleanup_replies.is_blackhole() {
    let stage = eff.stage("cleanup_replies", cleanup_replies).await;
    let cleanup = Cleanup::new(...);
    self.cleanup_replies = eff.wire_up(stage, cleanup).await;
}
```

The **standard approach** for verifying this behaviour (as currently used in `fetch_blocks`) is to assert a precise sequence of three `TraceMatch` items using `assert_trace_contains`:

```rust
assert_trace_contains(
    &running,
    &[
        tm_add_stage("fb-1", "cleanup_replies"),
        tm_wire_stage_state(
            "fb-1",
            "cleanup_replies",
            Cleanup::new(
                StageRef::named_for_tests("fb-1"),
                StageRef::named_for_tests("block_source"),
                StageRef::named_for_tests("peer_selection"),
            ),
        ),
        tm_state(
            "fb-1",
            |s: &FetchBlocks| s.cleanup_replies.name().as_str().contains("cleanup_replies"),
            "state with cleanup_replies child",
        ),
    ],
);
```

This sequence verifies:
- The `AddStage` effect for the child (using `tm_add_stage` with the parent stage name and the base child name).
- The `WireStage` effect together with the exact initial state the child was created with (`tm_wire_stage_state`).
- The parent stage’s updated `State` after wiring (`tm_state` with a predicate that checks the reference was updated).

These `tm_*` helpers (`tm_add_stage`, `tm_wire_stage_state`, `tm_state`) are provided by `pure-stage` (in `trace_match`) and re-exported / extended via `stages/test_utils.rs`.

This pattern is strongly preferred over exact `assert_trace` because:
- The generated child name is not known in advance.
- The test only needs to verify the *wiring event* and the resulting state, not the entire surrounding trace.
- It remains stable even when other unrelated effects occur between these key events.

**When to disable virtual mode**
Only turn virtual child stages **off** (the old behaviour) for the rare integration-style test that genuinely needs the child implementation to run inside the same simulation (e.g. to verify a full parent+child protocol).

All new stage tests, and the completion of existing incomplete suites, should use virtual child stages by default. The `setup` / `setup_preload` harnesses in each stage's `test_setup.rs` are expected to enable this mode unless the individual test explicitly requests real children.

### `tests.rs` — Structure and Style

Tests are kept in a separate `tests.rs` so that the voluminous scaffolding does not obscure the test cases.

**Typical layout:**

```rust
// ---------------------------------------------------------------------------
// MessageTypeOrControlFlow
// ---------------------------------------------------------------------------

#[test]
fn test_some_message_in_particular_state() {
    let prep = test_prep(...);           // world with the required external data
    let state = prep.state.clone();      // snapshot of the *before* state
    let msg = StageMsg::TheVariant(...);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    // Secondary: verify important log lines were emitted at the right level.
    // Every test must end with the strict triple below — INFO and above are public API.
    logs.assert_and_remove(Level::INFO, &["keyword", "another"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);

    // Primary oracle: the exact sequence of effects the stage performed
    assert_trace(
        &running,
        &[
            te_state("stage-short-name", &state),
            te_input("stage-short-name", &msg),
            te_load_header("stage-short-name", hash, true),
            te_send("stage-short-name", "manager", ManagerMessage::...),
            // any schedules, other sends, terminations, …
        ],
    );
}
```

**Guidelines for writing tests**

- The goal is **exhaustive control-flow coverage** of the stage’s `stage(...)` function. Every match arm and every significant `if`/`else` must have a test.
- Reaching a particular arm usually requires two things:
  1. An initial `State` manufactured by cloning `prep.state` and mutating the right maps/sets/counters.
  2. The exact `Msg` variant that the arm matches on.
- Use `setup_preload` when a test must inject a sequence of messages (e.g. to exercise cooldown timers or internal child stages).
- `assert_trace` is the most important assertion — it gives a deterministic, complete record of everything the pure-stage machinery observed.
- Log assertions (`Logs::assert_and_remove` / `assert_no_remaining_at`) are useful but secondary.
- **Every test must finish its `Logs` chain with `.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR])`.** Messages at INFO and above are part of the stage’s public observable behaviour (they may be consumed by operators, monitoring, or other components) and must be explicitly asserted. Use `assert_and_remove` for the messages you expect; the final strict triple guarantees that no unexpected INFO/WARN/ERROR was emitted.
- The short name passed to `network.stage("sc-1", ...)` becomes the name that appears in every `te_state`/`te_input`/`te_send` entry for that stage; keep it consistent within a test file.

### Shared Infrastructure — `stages/test_utils.rs`

Located at the `stages/` level (declared under `#[cfg(test)]` in `mod.rs`), it provides:

- `BufferWriter` and `Logs` — capture `tracing` output and offer convenient, chainable assertions that remove matched messages and finally verify (via `.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR])`) that no unexpected messages at INFO or above remain. Everything above DEBUG is considered public API and must be asserted in every test.
- Generic trace-entry constructors used by (almost) every stage:
  - `te_state(stage, &state)`
  - `te_input(stage, &msg)`
  - `te_send(from, to, msg)`
  - `te_terminate(at_stage)`
  - `te_terminated(at_stage, TerminationReason)`
  - `assert_trace(running, expected)`
- After the planned consolidation, all generic `te_*` helpers and the `assert_trace` implementation will live here. Stage-specific `test_setup.rs` files will only re-export or define the helpers that are unique to their external effects.

## Running the Tests

See the top-level `AGENTS.md` for the recommended cargo aliases and `make` targets. Typical commands:

```bash
cargo test -p amaru-consensus peer_selection
cargo test -p amaru-consensus --test tests validate_block
cargo test test_tip_not_found -p amaru-consensus
```

## Current Status

Many test suites are still incomplete and a few existing assertions are known to be incorrect or brittle. Completing and hardening the suites is planned future work. The structure described above is the **intended** target that new tests and refactored suites should follow.