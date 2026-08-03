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
- `src/model/`: event reduction and bounded UI state
- `src/ui/`: rendering, formatting, theme, and layout hotspots

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

## Useful commands

- `cargo check -p amaru-tui -p amaru`
- `cargo test -p amaru-tui`
