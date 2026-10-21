# Turbo optimization log

This log records changes that affect the CEK runtime's turbo benchmark. The
focused comparison uses 14 representative mainnet scripts, covering V1, V2,
and V3, data/list-heavy evaluations, byte-heavy evaluations, a large AST, and
the largest CPU outlier. Each script is evaluated ten times; the reported
figures are the arithmetic means from those runs. The arena figure is the
allocation high-water mark, not retained live heap.

The score weights CPU reduction at 75% and arena reduction at 25%. Complexity
is assessed qualitatively: a localized change with no new public abstraction
has cost 1; broader changes must earn a proportionally larger gain.

## Accepted

### Store builtin runtime state inside `Value::Builtin`

Partial builtin application previously allocated a `Runtime` object in the
arena in addition to the `Value` and argument node. The runtime is small and
already owned by the builtin value, so it is now stored inline. The argument
chain and call-site behavior remain unchanged.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Mean CPU | 18.60 ms | 17.82 ms | -4.2% |
| Mean arena occupancy | 8.16 MiB | 7.56 MiB | -7.4% |
| CPU-weighted score | | | 5.0 points |
| Complexity cost | | | 1 |

The change is accepted because it removes an allocation from every builtin
application and improves the CPU-weighted score without adding a new runtime
concept.

### Keep the current CEK state on the stack

The dispatch loop consumes each `MachineState` immediately, but the previous
implementation allocated every `Compute`, `Return`, and `Done` state in the
arena. The state is now moved through the loop by value. Contexts, values, and
environments remain arena-backed, so this does not change their lifetimes or
the machine's observable behavior.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Mean CPU | 17.82 ms | 12.53 ms | -29.7% |
| Mean arena occupancy | 7.56 MiB | 4.39 MiB | -41.9% |
| CPU-weighted score | | | 32.7 points |
| Complexity cost | | | 2 |

The extreme big-integer multiplication case did not improve materially; it is
dominated by arithmetic rather than CEK state transitions. The broad gain on
the remaining corpus and the absence of a new runtime abstraction justify the
small mechanical signature change.

## Rejected

### Intern primitive type descriptors

Returning shared descriptors for primitive `Type` variants reduced arena
occupancy by only 0.1-1.7% on the focused set and did not produce a repeatable
CPU improvement. It was reverted because the gain was below the cost of
maintaining another lifetime-sensitive global cache.
