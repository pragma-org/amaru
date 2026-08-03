# AGENTS.md for `amaru-tui`

This file narrows the root project guidance for work inside `crates/amaru-tui`.

Read the root `AGENTS.md` first. This file is the crate-local contract.

## Purpose

`amaru-tui` is an embedded observability surface for Amaru. It is not a control
plane, not a database client, and not a second source of truth for node state.

The crate should stay easy to maintain when:

- we change the UI
- we add or remove widgets
- telemetry schemas change
- runtime metrics change

If a change requires deep knowledge of ledger or consensus internals inside the
TUI, that is usually a design smell.

## Non-goals

Do not make the TUI:

- query databases or stores directly
- hold long-lived runtime handles into other subsystems
- maintain its own unbounded event history
- reconstruct hidden state through bespoke side channels

The normal data sources are:

- tracing events captured by `TracingLayer`
- shared metrics received through the metrics subscription
- immutable startup data passed through `StartupContext`

## Architectural boundaries

Keep this separation sharp:

- `capture.rs`
  - converts tracing records into TUI messages
- `metrics.rs`
  - subscribes to `amaru-metrics`
- `session.rs`
  - owns the thread, terminal loop, and lifecycle
- `startup.rs`
  - startup-only data such as process metadata, config sections, protocol
    parameters, and era history
- `model/`
  - folds messages into bounded UI state
- `ui/`
  - renders from the model only

The `ui` layer should not reach back into runtime code. The `model` should not
know about ratatui layout details unless strictly necessary.

## Maintenance rules

### When adding a widget

Ask these questions in order:

1. Can this widget be derived from existing telemetry?
2. If not, can it be derived from existing shared metrics?
3. If not, is it startup/static information that belongs in `StartupContext`?
4. If not, add telemetry or metrics at the producer side first.

Avoid bespoke TUI-only channels.

### When telemetry changes

Touch the crate in the following order:

1. `src/model/telemetry_event.rs`
2. `src/model/record_fields.rs`
3. the relevant reducer code in `src/model.rs` or its leaf modules
4. tests covering the affected behavior

Be suspicious of raw string matching. Reuse schema-generated constants from
`amaru-observability` whenever they exist.

### When metrics changes

Update:

1. the producer-side metric event shape
2. `src/metrics.rs`
3. the metric reducer paths in the model
4. tests that assert the derived UI state

Do not add a second metrics transport just for the TUI.

### When changing layout only

Prefer to keep the change inside:

- `src/ui/mod.rs`
- `src/ui/theme.rs`
- `src/ui/format.rs`
- `src/ui/views.rs`

Avoid mixing layout work with reducer or capture changes unless the feature
truly needs both.

## State and memory

The TUI must remain bounded and cheap:

- cap log/event retention
- cap scrollable collections
- prefer folding into summaries over retaining raw events
- avoid allocating large intermediate structures in the reducer path

If a widget needs history, keep the history window explicit and bounded.

## Styling and interaction

The current UI contract includes:

- three tabs: `Amaru`, `Cardano`, `Config`
- a splash screen during initial stake distribution loading
- an explicit copy mode toggled with `Esc`
- panel focus and scroll support

Preserve those interaction patterns unless the change is deliberate and
user-visible.

## Suggested workflow

For most TUI changes:

1. inspect the relevant producer event or metric first
2. update the reducer
3. update rendering
4. run focused tests
5. run a crate check against `amaru` too, because wiring changes often cross the
   crate boundary

Useful commands:

- `cargo check -p amaru-tui -p amaru`
- `cargo test -p amaru-tui`
- `cargo test -p amaru`

If you touch shared observability code, also check the producer crate.

## Code style reminders

- follow the root `AGENTS.md` and the EDRs
- match neighboring code exactly before introducing a new abstraction
- keep functions and types local to the most relevant module
- prefer one focused type per module when that improves navigation
- avoid comments unless explicitly asked
- no speculative abstractions

The best TUI code here is usually:

- small
- reducer-driven
- bounded
- easy to grep
- obviously derived from telemetry or metrics
