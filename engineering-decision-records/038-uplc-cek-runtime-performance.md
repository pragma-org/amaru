---
type: architecture
status: accepted
---

# UPLC CEK Runtime Performance

## Context

The UPLC evaluator is on a ledger-validation hot path. Its values, CEK states,
continuation frames, environments, and builtin applications can therefore
contribute both directly to execution time and indirectly to arena growth.

The turbo benchmark is used to compare changes against a fixed corpus of
mainnet-derived scripts. It contains representative V1, V2, and V3 scripts,
including data-heavy and byte-heavy evaluations, large ASTs, and CPU outliers.

## Decision

### Evaluating useful optimisations

To evaluate optimization across the CEK machine, without bloating the codebase
with AI-generated non-sense, we construct a heuristic that scores optimisations
based on their effectiveness and code complexity.

Optimizations are judged primarily by CPU improvement: CPU reduction
constitutes 75% of the weight, whereas memory reduction in the arena weights
25%.

Complexity is assessed qualitatively: a localized change with no new public
abstraction has cost 1; broader changes must earn a proportionally larger gain.

This keeps small but measurable improvements preferable to broad runtime
machinery with uncertain benefits.

### Retained optimisations

#### Keeping runtime state out of the evaluation arena where its lifetime permits

Builtin runtime state is stored inline in `Value::Builtin`. Partial builtin
application previously allocated a `Runtime` in the arena in addition to the
builtin value and argument node. The runtime is small and already owned by the
builtin value, so storing it inline removes one allocation without changing
argument ownership or call-site behavior.

CEK states are moved through the dispatch loop by value rather than allocating
every `Compute`, `Return`, and `Done` state in the arena. Contexts, values,
environments, terms, and constructor results retain their existing arena
lifetimes.

Active continuation frames are kept in a machine-owned stack rather than being
allocated permanently in the arena. Returning from a computation pops the
current frame, so the stack retains only active continuations while evaluation
results keep their existing lifetimes.

These changes preserve the arena for data that must survive evaluation while
avoiding arena retention for short-lived control state.

| Change                                             | CPU    | Arena  | Score | Complexity |
| ---                                                | ---:   | ---:   | ---:  | ---:       |
| Inline builtin runtime state                       | -4.2%  | -7.4%  | 5.0   | 1          |
| Keep CEK states on the stack                       | -29.7% | -41.9% | 32.7  | 2          |
| Keep continuation frames on a machine-owned stack* | -6.0%  | -64%   | 20.5  | 3          |

\* Excluding the arithmetic-dominated outlier. All three changes earned their
complexity under the CPU-weighted scoring heuristic.

#### Traverse de Bruijn environments iteratively

Environment lookup uses an equivalent loop instead of recursive calls for each
de Bruijn step. The environment representation is unchanged. This removes
call-frame overhead in deeply nested lookups without introducing another data
structure.

| Case                                 | Before   | After    | CPU    | Complexity |
| ---                                  | ---:     | ---:     | ---:   | ---:       |
| 13,882-term environment-heavy script | 16.45 ms | 13.95 ms | -15.2% | 1          |

Shallow-environment cases were unchanged, making this a targeted improvement
rather than a new general-purpose runtime abstraction.

#### Keep builtin costing lazy and shared per invocation

Cost arguments compute their execution-memory representation only when a cost
formula needs it, and cache that result for the duration of the builtin
invocation. This avoids sizing irrelevant arguments and prevents a value used by
both memory and CPU formulas from being measured twice.

The budget is still calculated and charged before any value-level fast path is
considered. Optimizations must not alter protocol-defined costs.

| Effect | Complexity | Budget behavior |
| --- | ---: | --- |
| Unused arguments are not sized; shared arguments are sized once | Low | Unchanged |

The benefit is workload-dependent, but the implementation adds no runtime
allocation or public abstraction.

#### Avoid per-application argument-vector allocations

Builtin arguments are represented as a chain of pointers to existing values
instead of a bump-allocated vector. Applying another argument adds one chain
node without copying all previously collected arguments. The public indexing
behavior and application order remain unchanged.

| Effect                                                    | CPU          | Arena                  | Complexity |
| ---                                                       | ---          | ---                    | ---:       |
| No growing argument vector allocation or repeated copying | Not recorded | Lower allocation churn | 1          |

#### Share list tails across `mkCons`

Runtime lists use flat slices for decoded constants and shared cons spines for
lists built incrementally by `mkCons`. This avoids copying the entire remaining
slice each time a list element is prepended, while preserving iteration,
indexing, tail, encoding, and equality behavior.

The focused benchmark evaluated a freshly built list 100 times for each size,
then averaged the 70 fastest measurements. The arena column measures the
additional arena occupancy during evaluation.

| List size | CPU before | CPU after | CPU    | Arena before | Arena after | Arena  |
| ---:      | ---:       | ---:      | ---:   | ---:         | ---:        | ---:   |
| 128       | 44.3 μs    | 46.4 μs   | +4.7%  | 201 KiB      | 128 KiB     | -34.8% |
| 512       | 332.1 μs   | 101.4 μs  | -69.5% | 1.52 MiB     | 512 KiB     | -67.1% |
| 2048      | 2.64 ms    | 320.7 μs  | -87.9% | 18.1 MiB     | 2.00 MiB    | -88.9% |

The geometric-mean improvement is 66.1% for CPU and 71.2% for arena
occupancy. With the 75% CPU and 25% arena weighting, this gives a score of
67.4 for an estimated complexity of 3. The change is therefore retained as a
targeted optimization. A 12-script turbo slice showed no aggregate improvement,
because ordinary scripts rarely construct long lists incrementally; it is not
representative of this workload.

#### Reuse empty slices for empty runtime containers

Empty constructor fields, empty constructor values, and empty frame fields use
the process-wide empty slice instead of allocating an empty bump-backed vector.
This preserves the existing slice-based representation and does not change
ownership or lifetimes.

| Effect                                    | CPU          | Arena                            | Complexity |
| ---                                       | ---          | ---                              | ---:       |
| Empty containers use a shared empty slice | Not recorded | No zero-length vector allocation | Negligible |

#### Discharge the final UPLC value lazily

Evaluation returns an `EvalResult` backed by the runtime `Value`. Callers can
check success or unit results without converting the complete runtime value
back into a UPLC term. Full conversion remains available through
`EvalResult::into_term()` for callers that need the resulting term.

| Caller                                     | Final discharge | Effect                                      | Complexity |
| ---                                        | ---             | ---                                         | ---:       |
| Validation paths using success/unit checks | Avoided         | No full result traversal or materialization | Low        |
| Callers using `into_term()`                | Preserved       | Existing full result behavior               | Low        |

The turbo benchmark explicitly calls `into_term()` when it needs to report a
result, so this optimization primarily benefits production callers that only
need success or unit checks.

## Rejected alternatives

### Intern primitive type descriptors

Returning shared descriptors for primitive `Type` variants reduced arena
occupancy by only 0.1--1.7% on the focused corpus and produced no repeatable
CPU improvement. The additional lifetime-sensitive global cache was not
justified.

### Remove the `Constant::Data` wrapper for runtime data values

A direct data variant passed conformance tests, but showed no stable CPU gain
and less than 0.5% lower arena occupancy in affected cases. The surrounding CEK
transitions and data/list operations dominate those measurements, so the extra
representation and compatibility path were reverted.

### Compare `PlutusData` with an explicit pair stack

An iterative equality implementation avoided recursive calls but regressed a
smaller V3 case from approximately 0.78 ms to 1.39 ms because each comparison
needed a heap-backed pair stack. The inconsistent gain did not justify the
allocation and implementation complexity.

### Short-circuit identical `PlutusData` nodes

Pointer-identity checks for data nodes and leaf payloads remained within
measurement noise on the focused cases. The extra hot-path branches were
reverted because no stable benefit was demonstrated.

## Consequences

- Short-lived CEK control state no longer accumulates in the evaluation arena.
- The public behavior and protocol-level budget accounting remain unchanged.
- The machine owns one active continuation stack, whose capacity can grow with
  evaluation depth and is released with the machine.
- Bigint-heavy scripts remain a separate optimization problem; replacing or
  retuning the bigint implementation is outside this decision.
- Future optimizations must report CPU and arena effects separately and include
  their complexity rationale.
- One large integer-multiplication outlier did not improve materially from the
  control-state changes. It is dominated by bigint arithmetic rather than CEK
  state transitions, so it must be assessed separately.

## References

- [Turbo benchmark corpus and runner](../crates/amaru-uplc/benches/turbo/)
- [UPLC runtime](../crates/amaru-uplc/src/machine/)
