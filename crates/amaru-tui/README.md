# Amaru TUI

`amaru-tui` is the terminal UI used by Amaru when running in an interactive
terminal. It is launched automatically by `amaru node run` when `--no-tui` is
not set.

The architectural rationale lives in
[EDR 030](../../engineering-decision-records/030-embedded-terminal-observability-ui.md).

## Crate layout

- `src/capture.rs`: captures in-process tracing records
- `src/session.rs`: owns the terminal thread and runtime wiring
- `src/startup.rs`: static startup context and config sections
- `src/model.rs`: root app model and tests
- `src/model/`: bounded submodels plus reducer slices grouped by concern
  - `interaction.rs`: keyboard, mouse, focus, pane toggles, paging
  - `telemetry_update.rs`: folds OTLP-derived telemetry into model state
  - `metrics_update.rs`: folds OTLP-derived metrics into UI state
  - `queries.rs`: derived read-only views over bounded model state
- `src/ui/mod.rs`: thin compositor and shell chrome
- `src/ui/common.rs`: shared border, scrollbar, gradient, and layout helpers
- `src/ui/format.rs` and `src/ui/theme.rs`: shared formatting and styling primitives

## Maintenance contract

Most changes should fall into one of two buckets:

- UI work: layout, formatting, focus, copy mode, shutdown mode, new widgets
- observability work: telemetry or metrics changed and the reducer must follow

The crate should not become a backdoor into Amaru internals. Prefer deriving
new state from telemetry or shared metrics over introducing direct database or
store access.

Embedded sessions use the same model and reducers. The only extra wiring is:

- an in-process tracing layer that forwards structured telemetry into the TUI
- a single in-process callback that mirrors local `MetricsEvent`s

Shared metrics should remain about the Amaru process itself and the small set of
host totals that are genuinely useful to export globally. Process memory
footprint is sampled by Amaru itself and exported through the normal system
metrics payload, so the TUI consumes one metrics stream.

The TUI keeps bounded state rather than user-selectable rolling time windows.
Throughput and peer timing widgets use exponential moving averages, rollback
widgets keep a bounded recent history, and the log pane keeps one ordered stream
backed by per-severity retention buckets plus a pre-filtered view for the active
level and target filters. Process and host resource gauges are simpler: they
render from the latest merged `SystemSample` snapshot rather than keeping
historical TUI-local copies.

For telemetry, prefer the schema-generated helpers exported by
`amaru-observability` for both event matching and field decoding. Avoid raw
field-name strings in reducers when a generated accessor exists.

When changing the TUI itself:

- keep `src/ui/mod.rs` thin; prefer pushing widget rendering into
  `src/ui/components/` and page layout into `src/ui/screens/`
- keep the root `Model` as the bounded state container and split update logic by
  concern under `src/model/`
- prefer adding local helper functions next to the screen/component that uses
  them over growing generic utility modules prematurely

## Useful commands

- `cargo check -p amaru-tui -p amaru`
- `cargo test -p amaru-tui`
- `cargo run -p amaru -- node run --network preview`
