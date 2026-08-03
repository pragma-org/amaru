---
type: architecture
status: accepted
---

# Embedded terminal observability UI

## Context

Amaru already emits structured telemetry and runtime metrics, but consuming that
information has so far required external tooling, separate dashboards, or ad hoc
inspection of logs. That makes the operator experience unnecessarily rough:

- a node can be healthy yet appear silent or opaque while bootstrapping
- important Cardano state, peer state, and local runtime state are spread across
  many events
- there was no built-in way to inspect the node interactively without adding
  yet another coupling point into consensus, ledger, or networking internals

At the same time, a terminal UI is only desirable if it remains cheap to
maintain:

- it must not become a second source of truth for runtime state
- it must not reach into databases or long-lived internal objects
- it must degrade gracefully when telemetry is filtered out
- it must stay in Amaru's compilation path so telemetry and metrics changes are
  caught early instead of drifting silently

## Decision

Amaru shall provide an embedded terminal UI through a dedicated `amaru-tui`
crate.

### The TUI is a consumer, not a control plane

The TUI derives its state from the same observability streams exposed to other
consumers:

- structured tracing events, captured through a tracing layer
- shared runtime metrics, captured through the existing metrics publication
  mechanism

The TUI may additionally receive a small startup context for static or
bootstrapping-time information such as:

- process metadata
- global parameters
- protocol parameters
- era history
- trusted peers
- runtime configuration values

That startup context is intentionally narrow and immutable. Beyond it, the TUI
must not query stores, databases, or runtime subsystems directly.

### Keep the crate isolated, but compile it with the product

The UI lives in its own crate so layout, event reduction, terminal management,
and rendering stay out of the node binary and its domain crates.

At the same time, `amaru-tui` remains part of the normal build of Amaru rather
than being a detached side project. This keeps observability breakage visible:

- telemetry renames or schema drift should surface while compiling or testing
  the workspace
- metrics changes should be reflected through the same crate boundaries used by
  the production binary
- UI-only work stays local to `amaru-tui`

The intended maintenance boundary is:

- domain crates emit telemetry and metrics
- `amaru-tui` captures and reduces them
- `amaru-tui` renders from its own model only

### Model the UI as a fold over events

The TUI maintains a bounded in-memory model that is updated from:

- telemetry records
- metric records
- local terminal events such as keyboard, mouse, focus, and scrolling

This keeps the architecture simple:

- `capture` converts tracing into TUI messages
- `metrics` subscribes to the shared metrics stream
- `model` folds incoming messages into UI state
- `ui` renders from the model
- `session` owns terminal lifecycle and the TUI thread

The TUI stores only the state necessary for rendering. It is not a persistence
layer and must not grow unbounded over time.

### Provide three operator-oriented tabs

The initial UI is split into three views:

- `Amaru`: node-local health, logs, peers, throughput, chain quality, and
  internal runtime state
- `Cardano`: ledger-facing information such as epoch progress, pots,
  governance proposals, and protocol version
- `Config`: runtime configuration plus selected global and protocol parameters

This split was chosen to keep each screen legible and to separate:

- node operations from chain state
- frequently changing data from mostly static configuration

### Keep the splash screen until the node is meaningfully ready

The TUI starts with a splash screen rather than an immediately interactive but
half-empty dashboard.

The splash screen exists to:

- avoid presenting mostly disabled widgets during startup
- make bootstrap progress explicit
- surface progress of the initial stake distribution calculations, which gate a
  meaningful portion of the downstream UI

The splash screen is intentionally narrow in scope:

- brand and identity
- startup progress
- no competing widgets

### Support an explicit copy mode

The TUI provides an explicit copy mode toggled with `Esc`.

This exists because terminal dashboards and mouse-rich interfaces often compete
with the user's desire to select and copy text. Copy mode makes that state
obvious and reversible without exiting Amaru.

While copy mode is active:

- the accent switches to a dedicated visual treatment
- the header indicates the mode clearly
- command hints reflect the reduced interaction surface

### Degrade gracefully under filtered telemetry

The TUI must not assume it sees every event. If the log or metric stream is
filtered:

- widgets may show limited information
- widgets may remain disabled
- missing data must not be synthesized from hidden side channels

That constraint is deliberate: it keeps the UI honest about what Amaru is
actually exporting.

## Consequences

- Amaru now ships with a built-in, operator-friendly observability surface.
- The TUI stays largely isolated from consensus, ledger, and networking code,
  which reduces maintenance cost.
- The product benefits from stronger pressure to keep telemetry and metrics
  coherent because the TUI depends on them directly.
- Some coupling still exists by design:
  - startup metadata is passed in explicitly
  - telemetry and metrics shape the UI contract
- Widgets must tolerate missing information because log filtering can hide the
  events they rely on.
- UI-specific changes should usually stay within `amaru-tui`, while telemetry
  or metrics changes should update both the producer and the TUI consumer.

## Alternatives considered

### Rely only on external dashboards

We could have left observability entirely to Jaeger, Prometheus, or plain logs.
This was rejected because:

- it gives a poor out-of-the-box operator experience
- it requires extra setup for basic inspection
- it does not help validate that Amaru's own telemetry is sufficient

### Query runtime subsystems directly from the TUI

We could have passed stores, ledger handles, or internal channels directly to
the UI. This was rejected because it would:

- create a second path to runtime state
- increase coupling to internal representations
- make the TUI harder to maintain and easier to break during refactors

### Keep the TUI outside the main build

We could have treated the UI as a separate binary or sidecar project. This was
rejected because it would reduce compile-time feedback and make telemetry drift
more likely to go unnoticed.
