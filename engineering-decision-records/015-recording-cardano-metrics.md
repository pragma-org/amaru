---
type: architecture
status: accepted
---

# Recording & Exposing Cardano Metrics

## Motivation

Amaru [observability](./007-observability.md) has already been discussed and documented previously. However, something we're missing is exposing domain-specific metrics for tools such as [nView](https://github.com/blinklabs-io/nview), a local monitoring tool for a Cardano node.

nView reads Prometheus metrics from a `/metrics` endpoint, so if we collect and expose the correct Prometheus metrics, nView should "just work" with some fine tuning. This not only allows node operators to use their existing infrastructure with Amaru, but also allows us to use other industry tools that rely on Prometheus.

## Decision

We will propogate metrics via stage events instead of passing around a large record to each stage to be mutated. This metrics stage will be agnostic to the specific metrics themselves, instead relying on a `Metrics` trait, similar to the following:

```rs
pub trait Metric: Send + Sync {
    /// Update underlying OTel instruments (counters, histograms, gauges).
    fn record(&self, meter: &opentelemetry::metrics::Meter);
}
```


This allows any stage (Consensus, Ledger, Mempool, etc.) to create their own metrics very easily, just passing around an `Arc<dyn Metric>`.

To maintain organization, we will use a convention where each stage has a `metrics.rs` that defines the relevant metrics which implement the `Metric` trait.

## Consequences

- Each component of the Amaru node can fully own its own metrics

- It's very simple to introduce new metrics, and the metrics stage doesn't have to change frequently

- We can expose a set of metrics that has some intersection with the metrics exposed by the cardano-node to prometheus, allowing existing tooling to support Amaru.

- One difficulty introduced by this trait decision is that it makes static analysis extremely difficult, whether by a human or a tool. The convention of using the `metrics.rs` file is an attempt to alleviate that, so one does not have to search the whole codebase for metrics.
