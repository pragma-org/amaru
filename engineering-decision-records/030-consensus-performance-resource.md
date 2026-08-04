---
type: architecture
status: accepted
---

# Consensus performance resource

## Context

[EDR-024][edr-peer-handling] sketched a shared `PeerPerformanceResource` fed by chainsync, blockfetch, and keepalive, and consumed by block-source selection and peer churn.
[EDR-026][edr-tracing] requires network-health observability at four processing points per header (first reception, first block request, first block reception, local adoption), plus fork-switch timing.
[EDR-007][edr-observability] / [EDR-015][edr-metrics] govern how those observations surface as spans, events, and metrics.

Consensus stages already form a pipelined pure-stage graph with deliberate back-pressure ([EDR-011][edr-simulation]).
Performance state is **cross-cutting**: many stages produce samples, few stages consume rankings, and the same semantic event (e.g. “header announced”) may update multiple aspectx (e.g. peer availability and header lifecycle).
Putting that state into stage messages would introduce new high-rate information flows to the stage graph, creating cycles and capacity coupling that would be difficult to design such that deadlock cannot occur.

## Decision

### Shared resource, not a pure-stage stage

Performance state lives in a pure-stage **resource** (`ResourcePerformance`), not as another stage in the back-pressured graph.

Stages interact only via `ExternalEffect`s constructed on `Performance` (e.g. `eff.external(Performance::record_header_announcement(...)).await`).
Recording effects enqueue work and complete immediately from the stage’s point of view; query effects (e.g. `select_peers_for_fetch`) await a oneshot reply.

State is owned by a dedicated **worker thread** (Tokio `current_thread` runtime + unbounded op channel).
That thread is an explicit secondary actor **outside** pure-stage capacity control: it serialises mutations of `PeerPerformance` and `HeaderPerformance` without holding locks on multi-thread runtime workers.

**Why not a pure-stage stage?**
pure-stage solves *bounded* information flow with rigorous back-pressure; designing acyclic bounded graphs is inherent cost of that guarantee.
Performance data are already bounded indirectly (they are derived from stage traffic that *is* back-pressured).
Folding them into inter-stage messages would add cycles and capacity coupling for information that is not the target of those resource bounds.
Crossing the boundary with `ExternalEffect` keeps the stage graph simple while still making every probe point visible in simulation traces ([EDR-011][edr-simulation]).

**Why not drop under load?**
Peer scores and claims drive fetch selection; losing them silently is not “graceful degradation” the way losing export of OpenTelemetry is.
Queue depth is monitored: sustained growth is a design/capacity failure and must fail loudly, not drop ops.
Telemetry emission that could block the worker (OTLP export) should remain decoupled from op processing so the worker stays within its latency budget.

**Why one resource for peers and headers?**
Several probe points are intrinsically dual-purpose.
A single semantic event (“header announced”, “block delivered”, …) updates peer claims/scores *and* header lifecycle timestamps, keeping call sites few and consistent.
Splitting queues or stages would force dual instrumentation for the same domain event.

### Two logical maps, one worker

| Component | Role |
| --- | --- |
| `PeerPerformance` | Per-peer **claims** (intersection / header / block delivery on the parent chain) and **scores** (EWMAs of header lag, block response time, bandwidth; counters for fetch success/timeout; keepalive RTT). |
| `HeaderPerformance` | Open **header lifecycles** (received / requested / downloaded) until a terminal outcome; optional in-progress **fork switch**. |

Unit tests exercise these types directly without spawning the worker.
Integration and stage tests install `ResourcePerformance` and assert effect traces.

### Event-oriented API

Stages emit domain events, not low-level map mutations, currently:

- **Peer / chain tips:** `record_intersection`, `record_header_announcement`, `record_rollback`, `record_block_delivery`, `record_fetch_failure`
- **Header lifecycle / forks:** `record_blocks_requested`, `record_block_valid`, `record_block_pruned`, `record_header_abandoned`, `record_header_rejected`, `record_fork_started`
- **Peer lifecycle:** `clear_peer_availability` (disconnect / no remaining live connection; scores kept), `forget_peer` (ban; scores and claims removed)
- **Horizon:** `prune_below(min_height, now)`
- **Queries:** `select_peers_for_fetch`, `peer_covers_fragment`, `direct_claimants`, `rank_peers_for_churn`, `scores`, `snapshot`

Timestamps use pure-stage `Instant` so simulation remains deterministic ([EDR-014][edr-time] for wall-clock vs monotonic concerns at the node boundary).

### Fetch selection and scoring

`fetch_blocks` selects covering peers via `select_peers_for_fetch` (coverage from claims, ranked by a score to be tuned over time).
If coverage is weak or the set is empty, the stage may fall back to all eligible connections.

### Lifecycle terminalisation and pruning

Header lifecycles must always reach a terminal outcome so the map won't grow without bound and so network-health observations close:

| Outcome | When |
| --- | --- |
| `ValidBlock` / `InvalidBlock` / `AbandonedBlock` | Chain selection / validation / better chain |
| Rejected header variants | Undecodable, invalid, duplicate, store error (often without a prior open lifecycle) |
| `Pruned` | Header height falls below the immutable horizon |

The immutable horizon is `tip.height − k` after anchor drag in `adopt_chain` (`drag_anchor_forward`).
Peer maps are also cleaned on connection end (`clear_peer_availability` when no live connection remains) and on adversarial ban (`forget_peer`).

### Relation to tracing and metrics

[EDR-026][edr-tracing] spans (`perf.header.forward`, `perf.blocks.fetch`, `perf.fork.switch`, …) remain the span-based story for distributed traces and operator debugging.
The performance resource complements that with:

- **decision state** (who can serve what; ranked peer sets);
- **closed lifecycle telemetry** (`perf.header.lifecycle` intervals, fork-switch outcomes): the worker produces pure payloads when a lifecycle terminates; the external-effect handler emits tracing events and optional metrics ([EDR-015][edr-metrics]) on the stage effect executor.

OpenTelemetry export may drop or lag under resource or connectivity pressure. That must not stall the performance worker or couple export failure modes to peer/header state. Therefore **no OTel/metric emission runs on the performance thread**.

Spans answer “what path did this header take?”; the resource answers “given everything we have seen so far, whom do we ask next?” and ensures every accepted header is accounted for even when never adopted.
Probe points should stay aligned: the same stage moments that open/close [EDR-026][edr-tracing] spans are the natural places to record performance events, avoiding divergent instrumentation.

## Consequences

- Consensus stages depend on `ResourcePerformance` being installed in pure-stage `Resources` (production and stage tests).
- Simulation tests assert performance effects in stage traces (`te_*` / `assert_trace*`); peer/header logic is also unit-tested without the worker.
- Op queue depth is a capacity invariant of the node design, not a soft buffer to shed load.
- Dropping the last `Performance` handle joins the worker after the channel closes; teardown should avoid doing that join on a multi-thread Tokio worker under a deep queue.
- Ranking and churn algorithms can evolve inside `PeerPerformance` without reshaping the stage graph, as long as the event/query API remains stable.
- Until keepalive RTT and churn ranking are wired (below), peer quality is incomplete relative to the network-spec intent described in [EDR-024][edr-peer-handling] (latency + bandwidth-based selection).

## Future work

1. **Keepalive RTT tracking** — call `record_keepalive_rtt` from the keepalive mini-protocol handler; fold `keepalive_rtt_ewma` into fetch ranking and churn badness (and into bandwidth estimation where response time includes RTT).
2. **Churn** — peer selection should demote/promote using `rank_peers_for_churn` (or successor) on a schedule, not only react to adversarial bans.
3. **Scoring policy** — replace provisional EWMA heuristics with an explicit, testable policy (document knobs; avoid silent retunes).
4. **Horizon / dual-connection edge cases** — keep pruning and clear/forget rules aligned with multi-connection peers (inbound+outbound) so availability is cleared only when no usable connection remains.

## Discussion points

Captured mainly from review of the performance-resource PR ([#1127](https://github.com/pragma-org/amaru/pull/1127)):

- **Stage vs resource:** a dedicated pure-stage for peer performance would make selection logic “simulatable as a stage,” but would also pull decision-critical data into the back-pressured graph and add cycles.
  The chosen compromise is: pure maps unit-testable + effect traces in stage simulation + worker as the serialised owner of live state.
- **Two queues (header vs peer):** considered for isolating “droppable” telemetry from “must not drop” peer data.
  Rejected in favour of one op stream and a hard capacity invariant: if the node cannot digest performance ops, the design is wrong.
  Header and peer updates also share inputs, so splitting would duplicate probe points.

[edr-observability]: ./007-observability.md
[edr-simulation]: ./011-deterministic-simulation-testing.md
[edr-time]: ./014-time-in-amaru.md
[edr-metrics]: ./015-recording-cardano-metrics.md
[edr-peer-handling]: ./024-peer-handling-infrastructure.md
[edr-tracing]: ./026-tracing-span-design.md
