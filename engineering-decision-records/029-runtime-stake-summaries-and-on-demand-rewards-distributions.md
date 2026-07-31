---
type: architecture
status: accepted
---

# Runtime stake summaries and on-demand rewards distributions

## Context

The ledger currently computes stake distributions for several downstream consumers:

- header validation and leader schedule checks need pool stake and total active stake
- governance ratification needs pool voting stake, DRep voting stake, and the silent-SPO fallback vote
- rewards calculation needs the full account-heavy stake distribution

These consumers do not all need the same amount of data. In particular, the
runtime hot path is retaining multiple full `StakeDistribution` values in
memory even though only rewards calculation needs the `accounts` map.

That `accounts` map is the expensive part:

- it can contain a very large number of stake credentials
- it is duplicated across snapshots
- it was being kept alive mostly to support rewards calculation later in the epoch

Yet, due to the rewards depending on a stake distribution from 3 epochs in the
past, we have been keeping full rewards calculations for longer than necessary.

At the same time, the ratification path has only one specific dependency on
that account map: when a stake pool operator does not vote, ratification falls
back to the DRep delegated by the pool reward account if it's abstain or
no-confidence (provided some other conditions are met). Yet, we need not the
entire accounts map to resolve that information...

## Decision

Amaru shall separate the full stake distribution from the runtime summary kept in memory.

### Two stake distribution shapes

The ledger now distinguishes between:

- `StakeSummary`: the full snapshot, including `accounts`;
- `StakeDistribution`: the slim runtime snapshot, containing only:
  - epoch and pots metadata
  - aggregate stake totals
  - pools
  - delegate representatives

Both are produced from the same internal builder so they cannot silently
diverge in how stake is computed. That means, for now at least, we still pay
the memory cost of computing _everything_ but only once.

### Precompute the silent-SPO fallback vote

`PoolState` now carries a `fallback_drep`.

This value is derived while building the stake snapshot from the reward account delegation that Conway uses when a pool operator does not cast an explicit vote.

That means:

- ratification no longer needs the full `accounts` map
- the fallback logic remains tied to the same snapshot that produced pool and DRep voting stake
- the extra runtime data is small and localized to pools

### Keep only slim distributions in the shared queue

The shared queue inside `State` now stores `StakeDistribution`, not `StakeSummary`.

The runtime keeps only the two latest stake distributions:

- the most recent one, for current leader-schedule style lookups
- the previous one, to tolerate the one-epoch delay involved in governance ratification and rollback-sensitive logic

Older full stake distributions are not retained in memory.

> [!NOTE]
>
> A non-obvious consequence of this is that we now only need two snapshots for bootstrapping. In fact, we had always only needed two.

### Rebuild the full distribution only when rewards need it

Rewards calculation now reconstructs the full `StakeSummary` directly from the
historical snapshot for epoch `e - 3`, at the moment rewards for epoch `e` are
computed. Before, we would re-use a previously computed summary held in memory.

This is an intentional CPU and disk-I/O tradeoff:

- memory is saved during the whole epoch
- the expensive account map only exists while rewards are actually being computed
- this roughly double the time it takes to compute rewards (which ultimately happen asynchronously and over multiple days!)

## Consequences

- The steady-state runtime memory footprint is lower (by about 400MB on mainnet) because the shared queue no longer retains duplicated account maps.
- Ratification is now explicit about the only account-derived datum it actually needs: the pool fallback DRep; which even simplifies the code.
- Rewards calculation becomes more self-contained because it reconstructs its own full input from historical snapshots instead of depending on a long-lived in-memory queue entry.
- The ledger pays an additional reconstruction cost when rewards start, but that cost is isolated to the rewards path and avoids carrying large maps for the rest of the epoch.
- The extra cost paid when computing rewards can be fully anihilated by moving the computation to a thread.

### Alternatives considered

#### Keep full distributions in memory and compress shared keys

- We considered keeping multiple full snapshots and reducing duplication with a more compact internal representation or interning. This was rejected for now because:
  - it adds a bespoke storage abstraction to the hot path
  - it complicates ownership and update rules
  - it still keeps account-heavy data resident for consumers that do not need it

#### Re-open account state during ratification

We could have kept the runtime summary slim and re-derived the silent-SPO fallback vote from accounts on demand during ratification.
This was rejected because it would have reintroduced an account-level dependency in the hot path and would have defeated the main memory objective.

#### Persist rewards inputs separately first

We also considered moving more of the rewards machinery directly to on-disk intermediate state.
That remains a possible follow-up, but it was deferred because the first-order win is simply to stop retaining full account maps in the runtime summary queue.
