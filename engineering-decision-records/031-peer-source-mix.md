---
type: architecture
status: accepted
---

# Peer source mix and connection malus

## Context

Peer selection maintains four outbound candidate sources: **static**, **shared** (peer-sharing), **snapshot** (big-ledger file), and **ledger** (live registrations).
A hard-coded waterfall (static → shared → snapshot → ledger) starves later sources whenever earlier ones can fill `target_upstream_peers` ([#1180](https://github.com/pragma-org/amaru/issues/1180)).

[EDR-030][edr-performance] stores connection `failure_count` and a sticky `adversarial` flag for peer-sharing filters, but `regulate_peers` only skipped cool-downs and already-outbound peers.
Failed dials were immediately eligible again; maintenance outages had no time-based rehab for outbound selection.

Admins need a **small formula** for the desired source mix that:

- does not lock the set of source names forever;
- handles peers that appear in multiple sources;
- composes with a **forbidden** cool-down (hard) and a **quality** signal (soft);
- lets connection failures fade even when the peer is not re-tried for a while.

## Decision

### Mix formula (`peer-mix`)

One string configures floors, proportional weights, and optional per-source malus half-lives:

```text
peer-mix = entry ("," entry)*
entry    = name floor? weight? decay?
name     = static | shared | snapshot | ledger   # registry; unknown ⇒ config error
floor    = "!" uint                               # minimum slots if eligible peers exist
weight   = "~" uint                               # proportionality (not numeric equality)
decay    = "@" duration                           # malus half-life for this source
duration = N"s" | N"m" | N"h" | N"d"
```

**Examples:**

```text
static!2@2h, shared~6@6h, snapshot~8@12h, ledger~4@24h
ledger~1@48h, static~0, shared~0, snapshot~0
```

Defaults when a field is omitted:

| Field | Default |
| --- | --- |
| `!floor` | `0` |
| `~weight` | `1` if the entry is present (including floor-only `static!2`); use `~0` to exclude a source from proportional fill |
| `@decay` | global `peer-malus-half-life` (default **6h**) |

**No** `=` for weights (reads as equality). **No** arithmetic, conditionals, or nested groups.

### Allotment and spill

For `open = target_upstream_peers − |outbound|` eligible peers:

1. Assign **floors** in declaration order, each capped by eligible count and remaining open slots.
2. Distribute any remainder **proportionally** by `~weight` (largest-remainder), only among sources with weight &gt; 0 and remaining eligible peers.
3. **Spill**: any still-unfilled open slots are filled in declaration order from sources that still have eligible peers (empty / short buckets never leave permanent holes when another source can fill).

### Multi-source membership

Each peer has a single **canonical origin** for mix accounting (fixed priority, independent of formula order):

`static > shared > snapshot > ledger`

A peer counts only toward its canonical source’s allotment.

### Hard forbid vs soft quality

| Rule | Kind |
| --- | --- |
| Active cool-down | **Hard** — never dial |
| Already in `outbound_peers` | **Hard** |
| Connection / protocol failure | **Soft** — raises **malus** |
| Adversarial | Cool-down is **hard** for the ban window; after that, dial is allowed. Sticky `adversarial` remains **only** for peer-sharing filters ([EDR-030][edr-performance]) |
| Observed scores (lag, fetch, …) | **Soft** — **goodness** for ranking |

### Lazy-decay connection malus

Performance stores `(malus, malus_as_of)` per peer (plus optional telemetry counters).

```text
malus(now, τ) = malus_as_of × 0.5^((now − as_of) / τ)
```

- **On failure / adversarial impulse:** evolve stored malus to `now` with the **global** half-life, add impulse, store `(value, now)`.
- **On outbound ranking:** compute `malus(now, τ_source)` from stored state **without** writing bucket-specific evolution (so different `@` half-lives can score the same peer differently).
- Decay proceeds even if the peer is never selected again until the next touch/event.

Impulses (policy constants, not admin surface v1):

| Event | Impulse |
| --- | --- |
| Connect exhausted | `+1` |
| Adversarial (with cool-down) | `+12` |

Peer-sharing uses the same malus axis: `ok_for_sharing` requires `advertisable`, `!adversarial` (sticky, permanent for sharing only), and `malus(now, τ_global) < share_threshold` (default `0.05`).

### Within-bucket selection

After allotting `n` slots to a source:

1. Eligible = canonical members of that source, not outbound, not cooling.
2. For each candidate compute  
   `score = goodness(scores) − λ · malus(now, τ) [+ never_connected_bonus]`  
   Never-connected / no Performance record: small positive **bonus** (exploration of fresh addresses).
3. Convert to sampling weights `w ∝ exp(score / T)` (temperature `T` fixed policy).
4. Draw **n** peers **weighted without replacement** (exploration, not pure top-n).

### Ownership

| Concern | Owner |
| --- | --- |
| Source pools, cool-downs, mix, allotment | Peer selection |
| Malus + scores + sticky adversarial for sharing | Performance resource ([EDR-030][edr-performance]) |
| Formula parse / registry | Peer selection (`PeerMix`) |

## Consequences

- Config grows a `peer-mix` string (CLI / env); invalid syntax fails at startup.
- `regulate_peers` becomes mix → quality-weighted sample, not a waterfall.
- Sharing rehab follows malus decay; adversarial peers stay non-shareable until a future explicit clear (not part of this decision).
- Adding a source means a new registry name + pool wiring; old formulas remain valid if they omit the new name (weight 0 for absent names).

## Default formula

```text
static!2@2h, shared~6@6h, snapshot~8@12h, ledger~4@24h
```

## Future work

- Admin knobs for impulses, `λ`, `T`, share threshold (if operators need them).
- Optional absolute score floor (“never dial if malus above X”) in addition to cool-down.
- Align churn demotion ([EDR-030][edr-performance]) with the same malus/goodness axes.

[edr-performance]: ./030-consensus-performance-resource.md
