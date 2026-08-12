---
type: architecture
status: accepted
---

# Embedding Amaru

## Context

Amaru was split so that `amaru-node` can be used as a library while the `amaru`
binary remains the operator-facing product. That split is incomplete: embedding
still forces product concerns (CLI, process lifecycle, TUI, log scraping for
progress) onto any out-of-tree consumer.

Two concrete drivers make the gap visible:

1. **E2E epoch sync** — scripts scrape JSON traces for `epoch_transition.compute`
   and kill the process. That is brittle and not an embedding API.
2. **Follow-the-chain applications** — e.g. notify when a set of on-chain
   addresses changes. There is no supported way to observe adopted transactions
   or hold ledger state without reaching into private stores.

The embedded terminal UI (EDR 030) is correct as a product surface, but must not
be required for library use. Observability setup must not reverse-depend on the
TUI.

## Decision

### Embedding boundary: `amaru-node` only

The supported dependency for running an Amaru node in-process is **`amaru-node`**.

- Embedders must not need the product `amaru` crate, CLI, lifecycle helpers, or
  `amaru-tui` to start, follow, and stop a node.
- Types required to construct config, run the node, and consume observations are
  **re-exported** from `amaru-node` (kernel network types, meter, config, events,
  etc.). Downstream code may still depend on lower crates for advanced work, but
  the happy path is one crate.
- Cold-start snapshot import is out of band: see **`amaru-bootstrap`**.

### Bootstrap: dedicated `amaru-bootstrap` crate

All bootstrap concerns (S3/CDN snapshot discovery and download, archive import,
chain-store seeding, cardano-node snapshot parsing used for import) live in
**`amaru-bootstrap`**.

- Product CLI and e2e tools depend on it explicitly when they need cold start.
- A long-running follower that starts from existing DBs never needs this crate.
- Network I/O for snapshots is not part of `amaru-node`.

### Observation is outside the stage graph; stop is outside too

Embedders own process lifecycle the same way `amaru` `main()` does: they build a
Tokio runtime, install optional tracing, start the node, and call
`request_abort` when *they* decide to stop.

The preferred embedding façade is **`NodeBuilder`**: network-aware defaults for
paths, era history, global parameters, magic, and peers; optional observers and
meter. Low-level **`build_and_run_node(config, runtime_handle)`** remains for
CLI and advanced callers. The Tokio [`Handle`] is always an **explicit**
argument (never ambient `Handle::current()` inside the library). Metrics are
optional (`Config::meter` / `NodeBuilder::meter`); unset means a default empty
meter.

The node does **not** implement stop-at-epoch or other application policies.
Observers fire; the outer app decides.

### Injected observers (typed application events)

`amaru-node` accepts optional observer hooks (callbacks / channels) installed at
node build time:

1. **Block lifecycle (adopt + undo)** — after a successful tip **roll-forward**
   commit path (volatile push, stable apply if any, epoch-transition flush),
   user code is invoked with a short-lived reference to the tip point, adopted
   transactions, and UTxO delta for that block (`LedgerBlockEvent::Adopted`).
   On a successful **fork switch**, the ledger first emits
   `LedgerBlockEvent::Undone` for each discarded volatile block (**tip-first**,
   UTxO delta only), then the adopt events for the new branch. Failed switches
   emit nothing. This is the primary hook for address watching and reorg-safe
   external indexes.
2. **Ledger state snapshots** — **opt-in**. The node deliberately avoids
   retaining large in-memory copies of full ledger state. When enabled, the
   outer app is shown a short-lived reference to snapshot material while the
   ledger still holds it. Aggregate `StakeSummary` / `StakeDistribution` are
   not `Clone`; clone individual fields and piece types (`AccountState`,
   `PoolState`, `DRepState`, map entries) when independent ownership (keep /
   index / other thread) is needed. The **query layer** remains the embedder’s
   responsibility; Amaru only supplies the means (events + optional field clones).
3. **Observability event subscription** — a simple interface to subscribe to
   structured tracing events defined by the typed schema in
   `amaru-observability`. This shares capture mechanics with the TUI so product
   and embedders do not diverge. Schema-oriented field access is preferred over
   ad hoc string scraping.

These hooks are **not** a control plane: observers must not drive consensus or
mutate node state except through documented APIs (e.g. mempool submit).
Callbacks run on the ledger path and block further ledger work until they
return; expensive processing should clone needed pieces and continue elsewhere.

### Observability crate direction

Dependency direction is:

```text
amaru-tui  →  amaru-observability
amaru (product)  →  amaru-tui (optional UI) + amaru-observability
amaru-node  →  amaru-observability (schema / re-exports as needed)
```

Never: `amaru-observability` → `amaru-tui`.

Product observability setup accepts optional local telemetry layers and metrics
observers without naming TUI types. The TUI (and embedders) supply layers built
from shared capture types, following the approach of exporting reusable
tracing-subscriber layers (see structured-logging work).

### Query layer

Address indexes, wallets, explorers, and other query models are **out of scope**
for Amaru core. Embedders build them on top of:

- adopted-transaction (and UTxO delta) events
- optional ledger snapshot copies
- on-disk stores they open themselves when appropriate

### TUI remains product-only

The TUI stays a telemetry consumer (EDR 030). Headless and embedded deployments
disable it. Unification of capture code with embedder subscription is intentional;
unification of *rendering* is not.

## Consequences

- E2E run-until becomes a thin program: bootstrap (optional) → run node →
  observe epoch progress → `request_abort`. No shell log scraping.
- Address notifiers become thin programs: run node → filter adopted UTxO
  deltas → notify. Email/SMTP stays outside Amaru.
- `amaru-node` gains a small, stable embedding surface and re-exports.
- Bootstrap weight and network dependencies stay out of the steady-state node
  crate.
- Product binary remains an orchestrator: CLI → config → node + optional
  OTEL/TUI/submit API on the same `build_and_run_node` path embedders use.

## Non-goals

- Full Cardano node query API (local state query protocol) as a first-class
  product in this decision.
- In-process stop policies (`stop_at_epoch` inside the stage graph).
- Requiring OpenTelemetry or the TUI for embedders.
