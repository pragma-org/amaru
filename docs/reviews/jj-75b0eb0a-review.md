# Review notes for `jj` commit `75b0eb0a`

Context: static code review only. I paused build/test checks on request, so the notes below are based on reading the diff and surrounding code.

## Summary

The change is directionally good: introducing `SimulationEvent` makes the simulation harness more general, and the new end-of-run property check is a sensible addition.

The main risk I see is not in the propagation logic itself, but in how failures are observed and reported. Right now the test can report a propagation failure even when the transaction was never injected successfully at the source node.

## Findings

### 1. Injection failures are downgraded to warnings, which can misreport the root cause

- `crates/amaru/src/tests/setup.rs:175` logs and continues when `MemoryPool::insert(tx, TxOrigin::Local)` fails.
- `simulation/amaru-sim/src/simulator/checks.rs:109` later checks only whether the expected tx ids are present in the node-under-test and upstream mempools.

Why this matters:

- If a downstream node fails to inject a tx locally, the final property check will still fail with `node ... did not receive all expected transactions`.
- That failure message points at propagation, but the real bug may be at the origin node during injection.
- In practice this makes the simulation much harder to debug, because the first hard failure happens far away from the actual cause.

Suggested fix:

- Make failed local injection a hard test failure, or
- record failed injections explicitly and surface them in `check_properties`, or
- at minimum include the source-node injection status in the final error.

### 2. The tx-submission assertion is too coarse for debugging failures

- `simulation/amaru-sim/src/simulator/checks.rs:113` only compares `txs.len()` with `expected_tx_ids.len()`.

Why this matters:

- When the assertion fails, you do not know which tx ids are missing, nor on which nodes they are missing.
- That is especially painful here because the new scenario injects multiple transactions per downstream peer.

Suggested fix:

- Compute and report the missing `TxId`s per node.
- If possible, include the downstream peer that originated each tx in the failure output.

### 3. The property currently verifies arrival, not full transmission semantics

- `simulation/amaru-sim/src/simulator/checks.rs:100` checks only the node under test and upstream peers.

Why this matters:

- The commit message says “check that all transactions are transmitted in a simulation”, but the implemented property is narrower: it checks end-state presence in selected mempools.
- That is a valid property, but it is not quite the same as “all transactions were transmitted”.
- For example, it does not tell you whether every downstream-originated tx was accepted locally first, whether it was relayed through each hop, or whether it merely exists in the final target mempools.

Suggested fix:

- Either tighten the commit/message naming to match the implemented property, or
- expand the property to check the full path explicitly (origin injection succeeded -> node under test received -> upstream peers received).

## Non-blocking notes

- `crates/amaru/src/tests/configuration.rs:212` keeps `with_actions` as a compatibility wrapper over `with_peer_actions`, which is a nice migration path.
- `simulation/amaru-sim/src/simulator/run_tests.rs:163` returning `(Vec<NodeTestConfig>, Vec<TxId>)` is a reasonable way to thread expectations into the checker without changing too much wiring.
- `crates/amaru/src/tests/nodes.rs:107` / `crates/amaru/src/tests/nodes.rs:141` still rely on bounded loops for progress/drain. That is fine for now, but if this test becomes flaky, the event scheduling and drain semantics would be the next place I would inspect.

## Suggested next steps when we resume

1. Decide whether failed tx injection should be fatal or merely recorded.
2. Improve the checker error output to list missing tx ids per node.
3. Optionally run a targeted `cargo check -p amaru-sim` and the relevant simulation test once you want validation resumed.
