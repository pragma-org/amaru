---
type: architecture
status: accepted
---

# Recording & Exposing Cardano Metrics

## Motivation

Amaru [observability](./007-observability.md) has already been discussed and documented previously. However, something we're missing is exposing domain-specific metrics for tools such as [nView](https://github.com/blinklabs-io/nview), a local monitoring tool for a Cardano node.

nView reads Prometheus metrics from a `/metrics` endpoint, so if we collect and expose the correct Prometheus metrics, nView should "just work" with some fine tuning. This not only allows node operators to use their existing infrastructure with Amaru, but also allows us to use other industry tools that rely on Prometheus.

## Decision

We will propogate metrics via a stage effect instead of passing around a large record to each stage to be mutated. There is a new `amaru-metrics` crate that holds the structs and enums relevant for the metrics collection.There is a `MetricsEvent` enum, with a variant for each specific type of metric (`LedgerMetrics`, `ConsensusMetrics`, etc.). Each of those metrics implements a `MetricRecorder` trait, which is called from `RecordMetricsEffect` effect.

## Discussion

We had previously discused using a generic `Metric` trait that a metrics stage could consume without carying about specifics using dynamic dispatch. This allowed the metrics to be owned by the relevant stage and modifying said metrics required no changes to any other pieces of logic.

While this worked well with the `gasket` framework, it was incompatible with `Pure Stage` due to the required `pure_stage::SendData` trait. As a result, we had to compromise and fallback to the `MetricsEvent` enum solution, which still limits the amount of changes needed to add/remove metrics.
