---
type: architecture
status: accepted
---

# Observability

## Context

This decision record documents a general organization for how the Amaru node will collect and report critical operational metrics, logs, and tracing data.

## Motivation

A key component of operating any computing system at scale is observability: tracking what happened, how often something happened, how long it took, how many resources something is consuming, and where potential issues or bottlenecks might be found.

Thus, we would like Amaru to track and report these traces and metrics. We would also like to use industry standards so that users of the Amaru node can swap out their own visualization and aggregation infrastructure as needed.

We would like the codebase to organize the metrics it tracks in a simple, consistent and modular way, such that each module or crate can own a subset of the metrics, while at the same time being consistent and discoverable through reading the code.

## Decision

### Observability

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

### Metrics

Each top-level module or crate will (optionally) define its own Metrics module, which exposes a `{CrateName}Metrics` struct, with a `new(..)` method and deriving `Clone`. At the time of this decision record, this would be the Amaru binary, Consensus, the Ledger, and the Sync module.

Each module or crate will, if applicable, accept an argument of type `{CrateName}Metrics` during initialization, and store an owned instance of this struct.

Each metrics struct will expose a number of top-level [Counters, Gauges, or Histograms](https://docs.rs/opentelemetry/latest/opentelemetry/metrics/index.html) relevant to its workload, and in the course of performing its work, will update these accordingly. The `new(..)` method will accept an [`SdkMeterProvider`](https://docs.rs/opentelemetry_sdk/latest/opentelemetry_sdk/metrics/struct.SdkMeterProvider.html) and initialize relevant objects with names, descriptions, and units as appropriate.

- [Counters](https://opentelemetry.io/docs/specs/otel/metrics/api/#counter) are used to count discrete or measurable events or quantities that can be accumulated over time, such as number of blocks processed, number of bytes read, etc.
- [Gauges](https://opentelemetry.io/docs/specs/otel/metrics/api/#gauge) are used to track measurements that only make sense at a point in time, and shouldn't be added, such as current Memory or CPU usage, temperature, etc.
- [Histograms](https://opentelemetry.io/docs/specs/otel/metrics/api/#histogram) are used to track values where the statistical distributions are important, such as response latencies, where you want to query the 90th percentile, etc.

Such metrics should follow the [Open Telemetry Metrics Semantics conventions](https://opentelemetry.io/docs/specs/semconv/general/metrics/).

The Amaru binary itself will contain a `metrics` module that stores and constructs instances of all other metrics types. It will be responsible for constructing and orchestrating all modules and crates. It will also track common system and process metrics, such as CPU and memory usage.

## Consequence

- Each component of the Amaru node can fully own its own metrics. At the same time, someone wishing to document or explore the metrics supported by the Amaru node has a single entrypoint to begin their exploration.

- Users of the Amaru node can track, aggregate, and alert on the health of the node based on multiple system or component specific metrics. They might notice, for example, that CPU usage hits 100% at epoch boundaries, resulting in missing slot leader checks, which indicates that the machine they are running the node on is underprovisioned.

- [PR #84](https://github.com/pragma-org/amaru/pull/84)
