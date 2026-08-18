---
type: architecture
status: accepted
---

# Tracing stack span encoding

## Motivation

Amaru emits the same `tracing` events onto four product stacks: console (stderr),
JSON NDJSON, OpenTelemetry (traces + logs), and the in-process TUI capture layer.
[EDR-007](./007-observability.md) and [EDR-026](./026-tracing-span-design.md)
define *what* we instrument. They do not define how a **nested span context**
is presented on each sink.

After the structured-logging work, JSON lines dropped the span name, console
hashes were diagnostic-quoted then Debug-quoted (`header_hash="\"abc…\""`),
and neither JSON nor the TUI recorded which span an event belonged to.

Repeating the full ancestor chain with every span's fields on every line
(the stock "full" fmt format) makes console lines too long and inflates
log volume. Operators already have process-local span ids: a child line can
point at its parent, and the parent's own enter/exit/close line is where
that parent's fields belong.

This record is the encoding contract. Unit tests in
`amaru-observability/tests/tracing_stacks.rs` emit one shared nested-span
fixture through each complete stack and assert the shapes below.

## Decision

### Shared principles

These rules apply to console, JSON, and TUI. OpenTelemetry keeps its native
span tree (see below).

1. **An event carries only its wrapping span.** That is the current span at
   the event (including synthetic `enter` / `exit` / `close` events). The
   event records that span's **name** and **target**. Ancestor spans are
   not expanded into fields.
2. **The wrapping span's fields are inlined with the event fields.** Event
   fields win if a key collides. Ancestor fields are not copied: they
   appear only on that ancestor's own lifecycle lines.
3. **`parents` is names only**, outermost → parent of the wrapping span.
   The wrapping span itself is already in `span.name` and is not repeated.
4. **Ids are how you walk up.** A span's own lifecycle events carry `.id`.
   Child events (and child-span lifecycle events) carry `.parent_id` equal
   to the wrapping span's *parent*. Search the log for that id to recover
   the next level up. OpenTelemetry uses its own span/trace ids.
5. **String scalars are not Debug-quoted.** Schema types such as
   `HeaderHash` travel as CBOR text (hex). Sinks present the hex once
   (`"3bc8…"`), never `"\"3bc8…\""`.
6. **Schema tags (`amaru.tag.*`) are filter attributes.** JSON and
   OpenTelemetry keep them. Console and TUI hide them.
7. **CBOR `record_bytes` decodes to the richest type the sink can hold:**
   nested JSON / OTEL log `AnyValue` for maps and arrays; homogeneous OTEL
   *trace* arrays when possible; diagnostic text otherwise.

### Console

Use [`CborConsoleEventFormat`](../crates/amaru-observability/src/layers.rs)
with `console_field_formatter()` (CBOR decode + tag hiding). Do **not** use
the stock full or compact formats.

The span stack is abbreviated like a Java logger name: each `.`-separated
segment keeps its first character, and levels are joined with `:`.
`epoch.transition` → `e.t`, so a level typically costs four characters
including the separator (`e.t:`).

```text
TIMESTAMP  INFO e.t:g.r amaru::ledger: ratification.round ratification.summarize is_dormant_epoch=false header_hash="3bc8…" votes=372 parent_id=6756…
TIMESTAMP  INFO e.t amaru::ledger: governance.ratify_proposals close epoch=598 id=6980… parent_id=6756…
TIMESTAMP  INFO amaru::ledger: epoch.transition close from=599 into=600 id=6756…
```

- Abbreviated path shows depth without repeating full ancestor names.
- The wrapping span's full name is printed so lines stay grepable.
- Wrapping-span fields are inlined after the event fields.
- Ancestor fields appear only on that ancestor's own `close` (or other
  lifecycle) line, together with `.id`.
- Child lines carry `parent_id=` so an operator can search upward.
- Tags do not appear.
- Product console still uses `FmtSpan::CLOSE` only, to keep volume down.

### JSON

Each NDJSON object has this envelope:

| Key | Meaning |
| --- | --- |
| `timestamp`, `level`, `target` | Event envelope. |
| `fields` | Event fields **plus** the wrapping span's fields (event wins on collision). |
| `span` | `{ "name", "target" }` of the wrapping span. Identity only. |
| `parents` | Array of ancestor span **names**, outermost first, excluding the wrapping span. |
| `id` | Present on span lifecycle events (`enter` / `exit` / `close`) only. |
| `parent_id` | Present when the wrapping span has a parent. |

Example (elided timestamp):

```json
{
  "level": "INFO",
  "fields": {
    "message": "ratification.summarize",
    "is_dormant_epoch": false,
    "header_hash": "3bc8…",
    "votes": 372
  },
  "target": "amaru::ledger",
  "span": { "name": "ratification.round", "target": "amaru::ledger" },
  "parents": ["epoch.transition", "governance.ratify_proposals"],
  "parent_id": 69806618857963521
}
```

The wrapping span's `enter` line has the same `parent_id` and also `"id": <wrap>`.
`governance.ratify_proposals` enter/close has `"id": 69806618857963521` and
`parent_id` of `epoch.transition`. The outer span's lifecycle has
`"id": 6756224074776578` and no `parent_id`. Ancestor fields (`from`,
`into`, `epoch`) live only on those ancestor lines.

### OpenTelemetry

OTEL already has a native parent/child span tree. We do not invent a
parallel encoding:

- Tracing span names become OTEL span names.
- Entered nested spans form a parent/child pair that share a `trace_id`.
- Span fields become attributes (homogeneous arrays when CBOR allows;
  diagnostic text otherwise). String scalars are the plain value.
- Tags remain attributes (`amaru.tag.cpu=true`) so backends can filter.
- OTEL **logs** produced by `CborOtelLogBridge` set `trace_context` from the
  current entered span so log records join the same tree. Event fields become
  log attributes / body; span fields stay on the span.

Spans that are created but never entered keep the documented
`CborTraceArrayLayer` limitation (CBOR upgrades apply on enter).

### TUI

`TelemetryCaptureLayer` records the same model:

- `parents: Vec<String>` — ancestor names, outermost first, excluding the wrapping span
- `span_name: Option<String>` — the wrapping span (`None` outside any span)
- wrapping-span fields inlined into `fields` on point events
- `id` on closed-span records; `parent_id` when the wrapping span has a parent

The TUI log line shows abbreviated ancestors plus the wrapping span's full
name (`e.t:g.r:ratification.round`), then the event label and fields. Tags
are omitted from `fields`. Schema matching continues to use `target` +
`name` (event or closed-span identity), not `parents`.

### Tests as the contract

`amaru-observability/tests/tracing_stacks.rs` composes each stack the way the
product binary does and asserts the shapes above against one fixture:

- outer `epoch.transition` (`from`, `into`, tag)
- mid `governance.ratify_proposals` (epoch, tag)
- wrapping `ratification.round` (CBOR hash, late `votes` record, tag)
- event `ratification.summarize` (bool + CBOR string array)

Treat a failing stack test as a break of this EDR, not as a snapshot to
refresh casually.

## Consequences

- Console lines stay short: abbreviated path + one full wrapping name +
  wrapping fields only. Log volume no longer grows with unused ancestor
  field bags.
- Reconstructing a parent span means searching for `id=<parent_id>` (JSON
  and console) rather than reading fields off the child line.
- JSON consumers that grepped flattened root keys such as `.epoch` must
  read `.fields.epoch` instead. Ancestry is `.parents` (names) plus
  `.parent_id`.
- Embedder `amaru-node::Telemetry` still uses stock `fmt` / `.json()` helpers.
  Embedders that want this contract should compose the same
  `amaru-observability` layers the product uses.
- Changing the encoding requires updating this EDR and the stack tests
  together.

## Discussion points

- Inlining wrapping-span fields into `fields` (instead of a fat `span`
  object plus root flattening) keeps one map and avoids duplicating keys.
- Abbreviating every path segment (`epoch.transition` → `e.t`) is preferred
  over printing a depth count (`^2`). Collisions such as `effects.timeout`
  vs `epoch.transition` are accepted; the full wrapping name sits next to
  the path for grep.
- Console emits `close` only. The span id is stable, so searching `id=`
  still finds the parent; JSON `enter` has the id earlier.

## References

- [Issue #1208](https://github.com/pragma-org/amaru/issues/1208)
- [EDR-007 Observability](./007-observability.md)
- [EDR-026 Tracing span design](./026-tracing-span-design.md)
- [EDR-030 Embedded terminal observability UI](./030-embedded-terminal-observability-ui.md)
