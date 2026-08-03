# Amaru TUI

`amaru-tui` is the embedded terminal UI used by Amaru when running in an
interactive terminal.

The architectural rationale lives in
[EDR 030](../../engineering-decision-records/030-embedded-terminal-observability-ui.md).

## Crate layout

- `src/capture.rs`: turns structured tracing into TUI messages
- `src/metrics.rs`: subscribes to the shared metrics stream
- `src/session.rs`: owns the terminal thread and runtime wiring
- `src/startup.rs`: static startup context and config sections
- `src/model.rs`: root app model and tests
- `src/model/`: bounded submodels plus reducer slices grouped by concern
  - `interaction.rs`: keyboard, mouse, focus, pane toggles, paging
  - `telemetry_update.rs`: folds structured tracing into model state
  - `metrics_update.rs`: folds shared metrics into time-series state
  - `queries.rs`: derived read-only views over bounded model state
- `src/ui/mod.rs`: thin compositor and shell chrome
- `src/ui/screens/`: page-level layout for `Amaru`, `Cardano`, `Config`, and `Splash`
- `src/ui/components/`: reusable panel renderers and widget-level view logic
- `src/ui/common.rs`: shared border, scrollbar, gradient, and layout helpers
- `src/ui/format.rs` and `src/ui/theme.rs`: shared formatting and styling primitives

## Maintenance contract

Most changes should fall into one of two buckets:

- UI work: layout, formatting, focus, copy mode, new widgets
- observability work: telemetry or metrics changed and the reducer must follow

The crate should not become a backdoor into Amaru internals. Prefer deriving
new state from telemetry or shared metrics over introducing direct database or
store access.

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
