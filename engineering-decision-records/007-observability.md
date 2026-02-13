---
type: architecture
status: accepted
---

# Observability

## Context

This decision record documents a general organization for how the Amaru node will collect and report critical operational metrics, logs, and tracing data.

A key component of operating any computing system at scale is observability: tracking what happened, how often something happened, how long it took, how many resources something is consuming, and where potential issues or bottlenecks might be found.

Thus, we would like Amaru to track and report these traces and metrics. We would also like to use industry standards so that users of the Amaru node can swap out their own visualization and aggregation infrastructure as needed.

We would like the codebase to organize the metrics it tracks in a simple, consistent and modular way, such that each module or crate can own a subset of the metrics, while at the same time being consistent and discoverable through reading the code.

## Decision

### General Principles

- By default, the application provides human-readable information on stderr in a pretty format (possibly TUI).

- Additionally, the application will embrace [observability](https://peter.bourgon.org/blog/2017/02/21/metrics-tracing-and-logging.html) and provide logs, traces, spans and metrics using Rust's [tracing](https://docs.rs/tracing/latest/tracing/index.html) ecosystem.

- All traces and spans can be made available on stdout in a structured format (JSON).

- All traces, spans and metrics can be made available via the OpenTelemetry Protocol (OTLP) on the default OTLP ports.
  - To ease its consumption, we provide at least two example setups for collecting and monitoring this telemetry:
    - [Jaeger](https://www.jaegertracing.io/)
    - [Grafana](https://grafana.com/) and [Tempo](https://grafana.com/oss/tempo/)

  - We will use the [opentelemetry-rust](https://github.com/open-telemetry/opentelemetry-rust) crate to collect and report traces and metrics.

  - We will make judicious use of [Spans](https://opentelemetry.io/docs/concepts/observability-primer/#spans) to expose the structured nature of the workload the Amaru node performs.

- We define the frontier between logs and traces by following a simple rule:
  - Any event at the DEBUG level or above is considered a log
  - Any event at the TRACE level is considered a trace

### Tracing Schemas

To ensure consistency and enable compile-time validation of tracing instrumentation, we use a schema-based approach implemented in the `amaru-observability` crate.

#### Schema Definition

Schemas are defined using the `define_schemas!` macro in a central location (`amaru-observability/src/schemas.rs`). They are organized hierarchically matching the crate/module structure:

```rust
define_schemas! {
    consensus {
        validate_header {
            EVOLVE_NONCE {
                required hash: String
            }
            VALIDATE {
                required issuer_key: String
            }
        }
    }
    ledger {
        state {
            APPLY_BLOCK {
                required point_slot: u64
                optional error: String
            }
        }
    }
}
```

Each schema declares:
- **Required fields**: Must be present as function parameters with matching types
- **Optional fields**: Supplementary context that can be recorded later

#### Function Instrumentation

The `#[trace]` macro instruments functions with compile-time validated tracing:

```rust
#[trace(consensus::validate_header::EVOLVE_NONCE)]
fn evolve_nonce(&self, hash: String) -> Result<Nonce, ConsensusError> {
}
```

The macro generates `#[tracing::instrument]` with consistent settings:
- `level = Level::TRACE` - all instrumented spans are at TRACE level
- `skip_all` - function parameters are not auto-captured (explicit recording required)
- `target` - derived from schema path (e.g., `"consensus::validate_header"`)
- `fields(...)` - all schema fields pre-declared as `Empty`

At compile time, the macro validates:
- The schema exists at the specified path
- Required fields are present as function parameters
- Field types match the schema declaration

#### Span Augmentation

The `trace_record!` macro records fields to the current span with a schema anchor:

```rust
#[trace(ledger::state::APPLY_BLOCK)]
fn apply_block(block: &Block) {
    // Record additional fields with schema context
    trace_record!(ledger::state::APPLY_BLOCK, block_size = block.size(), tx_count = block.transactions.len());
}
```

This macro is a lightweight way to add context to the current span without creating a new one. The schema constant anchors the recording and documents which schema these fields belong to.

#### Benefits

- **Compile-time safety**: Typos or type mismatches in field names cause compilation errors
- **Consistency**: All spans follow the same instrumentation pattern
- **Discoverability**: All schemas are defined in a central, documented location
- **Flexibility**: Functions can have additional parameters beyond schema fields

### Metrics

Each top-level module or crate will (optionally) define its own Metrics module, which exposes a `{CrateName}Metrics` struct, with a `new(..)` method and deriving `Clone`. At the time of this decision record, this would be the Amaru binary, Consensus, the Ledger, and the Sync module.

Each module or crate will, if applicable, accept an argument of type `{CrateName}Metrics` during initialization, and store an owned instance of this struct.

Each metrics struct will expose a number of top-level [Counters, Gauges, or Histograms](https://docs.rs/opentelemetry/latest/opentelemetry/metrics/index.html) relevant to its workload, and in the course of performing its work, will update these accordingly. The `new(..)` method will accept an [`SdkMeterProvider`](https://docs.rs/opentelemetry_sdk/latest/opentelemetry_sdk/metrics/struct.SdkMeterProvider.html) and initialize relevant objects with names, descriptions, and units as appropriate.

- [Counters](https://opentelemetry.io/docs/specs/otel/metrics/api/#counter) are used to count discrete or measurable events or quantities that can be accumulated over time, such as number of blocks processed, number of bytes read, etc.
- [Gauges](https://opentelemetry.io/docs/specs/otel/metrics/api/#gauge) are used to track measurements that only make sense at a point in time, and shouldn't be added, such as current Memory or CPU usage, temperature, etc.
- [Histograms](https://opentelemetry.io/docs/specs/otel/metrics/api/#histogram) are used to track values where the statistical distributions are important, such as response latencies, where you want to query the 90th percentile, etc.

Such metrics should follow the [Open Telemetry Metrics Semantics conventions](https://opentelemetry.io/docs/specs/semconv/general/metrics/).

The Amaru binary itself will contain a `metrics` module that stores and constructs instances of all other metrics types. It will be responsible for constructing and orchestrating all modules and crates. It will also track common system and process metrics, such as CPU and memory usage.

## Consequences

- Each component of the Amaru node can fully own its own metrics. At the same time, someone wishing to document or explore the metrics supported by the Amaru node has a single entrypoint to begin their exploration.

- Users of the Amaru node can track, aggregate, and alert on the health of the node based on multiple system or component specific metrics. They might notice, for example, that CPU usage hits 100% at epoch boundaries, resulting in missing slot leader checks, which indicates that the machine they are running the node on is underprovisioned.

## References

- [PR #84](https://github.com/pragma-org/amaru/pull/84) - Initial metrics implementation
- [PR #638](https://github.com/pragma-org/amaru/pull/638) - Tracing schemas and compile-time validation
