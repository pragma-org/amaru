# Monitoring

This document summarizes the various details regarding to monitoring Amaru. As a pre-requisite, it's important to note that Amaru leverages [OpenTelemetry](https://opentelemetry.io/) to emit traces & metrics. A compatible observability backend such as [Jaeger](https://www.jaegertracing.io/), [Grafana Tempo](https://grafana.com/docs/tempo/latest/) and/or [Prometheus](https://prometheus.io/) is therefore needed to collect and visualise telemetry.

We provide example configurations using different compositions of tools:

- [Jaeger + Prometheus](./jaeger) _(simple)_
- [Grafana + Tempo + Prometheus](./grafana-tempo) _(more advanced)_

To turn on monitoring, use the following CLI options when running the application:

- `--with-open-telemetry` (or env variable `AMARU_WITH_OPEN_TELEMETRY`) to enable [OpenTelemetry](https://opentelemetry.io/) traces
- `--with-json-traces` (or env variable `AMARU_WITH_JSON_TRACES`) to enable JSON traces on stdout

## Filtering traces

Any event (trace, span or metric) can be filtered by target and severity using two environment variables:

- `AMARU_TRACE`: for any event emitted by the OpenTelemetry layer (enabled both by `--with-open-telemetry` and `--with-json-traces`);
- `AMARU_LOG`: for any event emitted to stdout;

> [!TIP]
> Both environment variable are optional.
>
> - When omitted, `AMARU_TRACE` defaults to all **amaru** and **pure-stage** targets above the **trace** level;
> - When omitted, `AMARU_LOG` defaults to all **errors**, **amaru** targets above the **debug** level, and **pure-stage** above the **warn** level;

### By target

A `target` is a `::`-separated path of identifiers such as `amaru::ledger::state`. One can filter by providing either a full target, or a sub-path prefix. For example, the target `amaru::ledger` will match the following:

- `amaru::ledger::state`
- `amaru::ledger::state::forward`
- `amaru::ledger::store`

But it will not match any of the following:

- `amaru::sync`
- `amaru::consensus`

e.g. `AMARU_LOG="amaru::ledger::state::forward=info"` will filter out `target` **amaru::ledger::state::forward** with level bellow `info`.

For a comprehensive list of available targets, spans, and traces, see [TRACES.md](../docs/TRACES.md).

### By severity

It is also possible to filter events by severity: `error`, `warn`, `info`, `debug`, `trace`, `off`. Severity can be specified either globally (in which case it applies to all events) or for a specific target by specifying the severity after the target using `=`. For example, `amaru::ledger::state=error` will filter out any events below the error severity for the `amaru::ledger::state` target.

### By span

A `span` name can be used as a filter too. Note that any `span` or `event` inside this `span` will be considered, including those not matching the initial `target` (e.g. `pallas` events could match).
For example `amaru[find_intersection]=trace` will filter all `spans` and `events` with the name `find_intersection` plus all children of this event.

### Combining filters

Filters can be provided as a sequence of `,`-separated values. Right-most filters take precedence. A usual pattern is to first define a global filter and override it with specific target. For example, `error,amaru::ledger::store=debug` will exclude any event below the `error` severity except those targetting `amaru::ledger::store` which will show up to the `debug` severity.

## Spans

A trace is a collection of spans logically connected together. A span represents a unit of work or operation in the system.

### Span Format

Each span consists of:
- **target**: The module hierarchy (e.g., `consensus::chain_sync`)
- **name**: The lowercase span identifier (e.g., `find_intersection`)
- **level**: The trace level (e.g., `TRACE`, `DEBUG`, `INFO`)
- **required_fields**: Fields that must be present in the span
- **optional_fields**: Fields that may optionally be present in the span

### Filtering by Span Name

You can filter by span name using square brackets:

```bash
AMARU_TRACE="[find_intersection]=trace"
```

For a comprehensive list of all available spans, see [TRACES.md](../docs/TRACES.md).

## Metrics

Coming soon.

> [!NOTE]
> The plan so far is to maximise compatibility with the existing Haskell node Prometheus metrics such that tools like [`gLiveView`](https://cardano-community.github.io/guild-operators/Scripts/gliveview/?h=gliveview) and [`nview`](https://github.com/blinklabs-io/nview) keep working out-of-the-box.
>
> We are planning, however, to add more metrics to Amaru.

## Configuring OpenTelemetry

Amaru recognizes standard OpenTelemetry env variable for its configuration:

- `OTEL_SERVICE_NAME`: Sets the `service.name` key used to identify metrics and traces. This is useful when a single OTLP service stack collects telemetry from several Amaru instances
- `OTEL_EXPORTER_OTLP_ENDPOINT`: Sets the endpoint used to send spans, defaults to `http://localhost:4317`
- `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`: Sets the endpoint used to send metrics, defaults to `http://localhost:4318/v1/metrics`

Note that two different transports are used internally:

- OTLP/gRPC for spans
- OTLP/HTTP for metrics

This helps maximize compatibility with 3rd party tools receiving those data.

One can find more available env variables [here](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/) and [here](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md).
