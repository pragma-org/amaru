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
| `PeerPerformance` | Per-peer **claims** (intersection / header / block delivery on the parent chain), **scores** (EWMAs of header lag, block response time, bandwidth; counters for fetch success/timeout; keepalive RTT), **share-relevant reputation** (`ever_connected`, latest handshake advertisability, connection failure count, sticky adversarial flag), and the **outbound candidate pools** (static, shared, snapshot, ledger) with the admin peer-mix. Learned addresses are capped at 4096 and per-peer records at 8192. Rows untouched for 24 hours are evicted, except a live bearer, a ban stub, a static, ledger, or snapshot candidate, and a peer that is bound or being dialled. |
| `HeaderPerformance` | Open **header lifecycles** (received / requested / downloaded) until a terminal outcome; optional in-progress **fork switch**. |

Unit tests exercise these types directly without spawning the worker.
Integration and stage tests install `ResourcePerformance` and assert effect traces.

Performance is **cross-stage memory** for observations that many stages produce and few consumers need (fetch ranking, churn, peer-sharing filters).
It owns the outbound candidate pools (static, shared, snapshot, and ledger) and the admin peer-mix, so connection malus decays with that source’s half-life ([EDR-031](./031-peer-source-mix.md)).
Cool-downs and listen-address policy remain in peer selection.
Successful connection is tracked by an explicit sticky `ever_connected` flag set on handshake; map presence alone is not sufficient (connection failures also upsert a reputation stub).

### Event-oriented API

Stages emit domain events, not low-level map mutations, currently:

- **Peer / chain tips:** `record_intersection`, `record_header_announcement`, `record_rollback`, `record_block_delivery`, `record_fetch_failure`
- **Header lifecycle / forks:** `record_blocks_requested`, `record_block_valid`, `record_block_pruned`, `record_header_abandoned`, `record_header_rejected`, `record_fork_started`
- **Peer lifecycle:** `record_advertisability` (successful handshake: sets `ever_connected`, records peer-sharing willingness; overwrites advertisability), `record_connection_failure` (increments failure counter telemetry, raises connection malus; does not set `ever_connected`), `clear_peer_availability` (disconnect / no remaining live connection; scores and reputation kept, claims cleared), `peer_adversarial` (adversarial ban: claims and scores cleared, entry retained with `adversarial = true` and prior `ever_connected` / `failure_count` / last `advertisable`, plus adversarial malus impulse; not a generic erase)
- **Horizon:** `prune_below(min_height, now)`
- **Queries:** `select_peers_for_fetch`, `peer_covers_fragment`, `direct_claimants`, `rank_peers_for_churn`, `scores`, `share_flags`, `snapshot` (includes share flags), `ok_for_sharing(now)`, `outbound_weights`

Timestamps use pure-stage `Instant` so simulation remains deterministic ([EDR-014][edr-time] for wall-clock vs monotonic concerns at the node boundary).

### Fetch selection and scoring

`fetch_blocks` selects covering peers via `select_peers_for_fetch` (coverage from claims, ranked by a score to be tuned over time).
If coverage is weak or the set is empty, the stage may fall back to all eligible connections.

The first request often sees only the peer who announced first. `fetch_blocks` schedules further asks at 30ms, 80ms, and 150ms after that request. Each wakeup is one query for covering peers not already asked; announcements themselves are not stage messages. Widening continues until every block in the batch has arrived, so a peer that returns only a prefix does not stop the later asks. The 5s batch timeout is unchanged.

### Lifecycle terminalisation and pruning

Header lifecycles must always reach a terminal outcome so the map won't grow without bound and so network-health observations close:

| Outcome | When |
| --- | --- |
| `ValidBlock` / `InvalidBlock` / `AbandonedBlock` | Chain selection / validation / better chain |
| Rejected header variants | Undecodable, invalid, duplicate, store error (often without a prior open lifecycle) |
| `Pruned` | Header height falls below the immutable horizon |

The immutable horizon is `tip.height − k` after anchor drag in `adopt_chain` (`drag_anchor_forward`).
Peer **claims** are cleared on connection end (`clear_peer_availability` when no live connection remains).
Adversarial ban uses `peer_adversarial`, which clears claims and scores but **retains a reputation stub** (`ever_connected`, `adversarial`, `failure_count`, last `advertisable`, malus) so peer-sharing and reconnection policy can still see the ban. A future plain-forget (drop memory without implying adversarial behaviour) would be a separate operation.

### Peer-sharing reputation (Performance half)

Peer-sharing reply filters need observations that span handshake, connection attempts, and bans.
Performance stores the reputation half and the origin pools. Share selection in this resource excludes ledger and snapshot peers and draws the sticky sample. Peer selection still applies listen-address rules.
Connection quality for dial and share rehab uses lazy-decay **malus** ([EDR-031](./031-peer-source-mix.md)).

| Flag / rule | Owner | Notes |
| --- | --- | --- |
| `ever_connected` | Performance | Sticky; set on successful handshake only (not by connection-failure upserts); sharing requires true |
| `advertisable` | Performance | Latest handshake wins (`VersionData.peer_sharing == 1`) |
| `failure_count` | Performance | Lifetime connect-failure counter (telemetry); soft policy uses malus |
| connection malus | Performance | Lazy half-life decay; sharing requires evolved malus below threshold (see EDR-031) |
| `adversarial` | Performance | Set sticky by `peer_adversarial`; sharing requires false (outbound may dial after cool-down) |
| Not ledger / not snapshot (big-ledger) | Performance | Origin pools live in `PeerPerformance`; share selection excludes ledger and snapshot peers |
| Known listen address (not pure inbound) | Peer selection | Inbound remote port is not a listen advertisement; optional outbound probe (~3000) may promote a peer later |

`ok_for_sharing(now)` / `share_flags` / `outbound_weights` expose the Performance half so peer selection can compose share filters and mix sampling without duplicating counters.

### Relation to tracing and metrics

[EDR-026][edr-tracing] spans (`perf.header.forward`, `perf.blocks.fetch`, `perf.fork.switch`, …) remain the span-based story for distributed traces and operator debugging.
The performance resource complements that with:

- **decision state** (who can serve what; ranked peer sets; share-relevant reputation);
- **closed lifecycle telemetry** (`perf.header.lifecycle` intervals, fork-switch outcomes): the worker produces pure payloads when a lifecycle terminates; the external-effect handler emits tracing events and optional metrics ([EDR-015][edr-metrics]) on the stage effect executor.

OpenTelemetry export may drop or lag under resource or connectivity pressure. That must not stall the performance worker or couple export failure modes to peer/header state. Therefore **no OTel/metric emission runs on the performance thread**.

Spans answer “what path did this header take?”; the resource answers “given everything we have seen so far, whom do we ask next?” and ensures every accepted header is accounted for even when never adopted.
Probe points should stay aligned: the same stage moments that open/close [EDR-026][edr-tracing] spans are the natural places to record performance events, avoiding divergent instrumentation.

### Message versus resource

The choice is timeliness, not reliability.

A message is required when the receiver must act immediately: which peer to ask for the next header or block, which chain to adopt, or an adversarial disconnect. Those stay stage messages. Population bookkeeping can wait about a second: a connection opened, a dial that failed, the local use applied on a bearer, addresses learned by peer sharing, a share request that was served, a keep-alive round trip, and a chainsync intersection that was not found. Those are written to this resource. The consumer reads them on its own schedule. Closing a bearer is written the same way. When that bearer was the peer's last one, the write clears the peer's claims immediately; only the later refill of outbound slots waits for the consumer. An intersection miss stops that chainsync session and clears availability in the same turn; the demotion of local use waits for the next selection tick. It does not choose the next header or block.

An observation that can change the next header or block request, or which chain is adopted, still has to reach that consumer within about 10 ms. A resource read is acceptable for that work only when the consumer is woken on change or ticks at about that rate. A one-second tick is not.

| Class | Observation | Budget |
| --- | --- | --- |
| C1 | Chain-sync roll forward or backward, through header validation, chain selection, and the block-fetch decision | immediate, at most 10 ms per hop |
| C2 | Block-fetch completion, through adoption and the next fetch decision | immediate |
| C3 | Fetch failure, timeout, or loss of a peer that holds in-flight requests | immediate |
| C4 | Local adoption, through roll-forward to waiting followers | immediate |
| C5 | Invalid header or block, through disconnect | immediate |
| C6 | Genesis density verdicts and the resulting disconnect. No instance in this node today | about 1 s |
| C7 | Keep-alive round trip, stored as the latest sample and an EWMA and readable on demand. Fetch and churn ranking do not consume it yet | seconds |
| C8 | Peer lost, through churn and refill | seconds or longer |
| C9 | Peer-sharing results | minutes |
| C10 | Transaction-submission statistics. No instance in this node today. A share-request window is peer-sharing bookkeeping (C9), not a submission statistic | seconds or longer |

Protocols reach the population half through the `PeerTracking` trait (`amaru-ouroboros-traits`). `Performance` implements that trait by enqueueing on this same worker, and the node registers that one handle under both resource names. Consensus stages keep using the existing `Performance` effects. track_peers records an intersection miss through the trait, one write on that same worker. The trait methods are the protocols-facing names, plus that miss.

The manager writes connection established, closed, connect-failed, and local-use-applied, one worker operation per event. Connection-failure malus is applied only inside the manager's connect-failed write. Peer selection does not receive connection lifecycle messages. It keeps one timeout, armed for the earlier of one second and the next stored deadline, and re-arms that timeout at the end of every turn. On each wake it asks the resource for a peer view since the generation it last saw. The resource generation advances on each lifecycle write, when ledger candidates are replaced, when a share ingest adds candidates, and when an intersection-not-found mark is recorded. An unchanged generation returns no view. The view carries the live bearers, the latest connect failure of each peer, the latest close of each peer, and marks newer than that generation and nothing older, only the latest mark per peer. A mark is dropped when its bearer is gone. A dial whose connection closed before this tick is dropped on the next round and is not held off; only a connect failure or a lost dial sets that hold-off. A full round runs when the view changed, a stored deadline is due, or thirty seconds have passed since the last full round. The round reconciles desired use with the view, then refills outbound slots. On a mark for a live Using bearer it sets the desired use to Maintenance and a demotion deadline of 120 seconds, or 180 seconds when the mark follows a rollback. `SetLocalUse` goes out on the same rate-limited path as any other desired-use change. A repeated mark while the bearer is already in Maintenance does not move that deadline. A mark whose bearer is already gone is not stored and does not move the generation. The deadline lifts in the round where it is due. A ledger-candidate replacement bumps the generation, so the next tick refills; peer selection has no separate refill message. Repeated adversarial reports while a ban is active are ignored apart from a debug log. The first report records the ban, writes the adversarial mark, and removes the peer; the refill waits for the next round. The peer-sharing responder serves a request with `query_share_peers` and records it with `record_share_request_served`. The initiator, started with the outbound maintenance group when the remote side is advertisable, writes each reply with `record_shared_peers`. A reply that adds candidates bumps the generation, so the next selection tick refills. The keep-alive initiator records each matching round trip with `record_keepalive_rtt`, one enqueue, and does not wait for the worker to apply it. The resource keeps the latest sample and an EWMA on that peer's scores and copies them out on demand. Fetch ranking and churn ranking do not read that summary, and the write does not bump the generation. Each inbound share request the responder answers is counted by `record_share_request_served`, including how many fell in the current window and the previous one. Those counts are not enforced, and the reply is unchanged.

### Retention and caps

Activity is the latest instant a peer was observed: a handshake, a close, a dial failure, a dial note, a local-use update, a header or block claim, a fetch result, a keep-alive sample, a share request this node answered, a share ingest (the donor), an intersection-not-found mark, or an adversarial mark. That instant only moves forward.

A shared address is as fresh as the later of when it was first learned and that activity. Repeating a share reply does not move the learned instant.

Peer selection stores an eviction deadline of five minutes and folds it into the single timeout. When the deadline is due, the stage enqueues one eviction and does not wait for the worker to finish it. Each eviction examines at most 256 entries, using an oldest-first index and a cursor, so a full pass is never one operation.

A per-peer record is evicted only when it has no live bearer, no ban stub, and is not static, not a ledger or snapshot candidate, and not bound or being dialled, and either it has been untouched for 24 hours or the record map is over its cap and the peer is not established. The ban stub lasts 24 hours after the adversarial mark. While the stage still protects that peer, each eviction extends the stub by another 24 hours, so it outlasts the ban and a re-offender is still recognised. Established means a successful handshake, a score change, a keep-alive sample, or a fetch success or timeout.

Learned addresses are capped at 4096. Per-peer records are capped at 8192. On ingest, a share reply that would pass the learned-address cap drops the new addresses and leaves addresses already learned in place. An unknown peer's share-request row is dropped when the record cap is already reached. A live bearer is recorded even if that briefly exceeds the record cap. The sweep then drops the oldest unprotected unverified entries until the cap holds. Established peers stay until the retention period, so a flood of share replies cannot push them out.

Eviction removes that peer's scores, keep-alive RTT, share-request row, claim leftovers, and intersection mark. It does not bump the resource generation; the next selection round reads the pools again.

## Consequences

- Consensus stages depend on `ResourcePerformance` being installed in pure-stage `Resources` (production and stage tests).
- Simulation tests assert performance effects in stage traces (`te_*` / `assert_trace*`); peer/header logic is also unit-tested without the worker.
- Op queue depth is a capacity invariant of the node design, not a soft buffer to shed load.
- Dropping the last `Performance` handle joins the worker after the channel closes; teardown should avoid doing that join on a multi-thread Tokio worker under a deep queue.
- Ranking and churn algorithms can evolve inside `PeerPerformance` without reshaping the stage graph, as long as the event/query API remains stable.
- Fetch ranking and churn badness do not yet read the keep-alive summary. Until that fold, peer quality is incomplete relative to the network-spec intent described in [EDR-024][edr-peer-handling] (latency + bandwidth-based selection).
- A peer-sharing reply is the resource sample. The responder calls `query_share_peers` and sends that list. The amount cap and the requester seed stay in the sample. The initiator writes learned addresses with `record_shared_peers`. New candidates bump the generation, and peer selection refills on its next tick.

## Future work

1. **Keepalive RTT in ranking** — the latest sample and `keepalive_rtt_ewma` are recorded and copied out. Folding them into fetch ranking and churn badness (and into bandwidth estimation where response time includes RTT) changes which peer is asked for a block, so it is a separate decision.
2. **Churn** — the selection round ranks copied score rows and demotes the worst non-static Using peers when the stored churn deadline is due.
3. **Scoring policy** — replace provisional EWMA heuristics with an explicit, testable policy (document knobs; avoid silent retunes).
4. **Horizon / dual-connection edge cases** — keep pruning and clear/forget rules aligned with multi-connection peers (inbound+outbound) so availability is cleared only when no usable connection remains.
5. **Failure-count decay** — superseded by connection **malus** with lazy half-life decay ([EDR-031](./031-peer-source-mix.md)); telemetry may still keep a raw failure counter.

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
