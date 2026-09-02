# Monitoring

Amaru emits metrics, logs, and spans through [OpenTelemetry](https://opentelemetry.io/). The monitoring directory provides one local observability stack: an OpenTelemetry Collector routes metrics to Prometheus, logs to Loki, and spans to Tempo, while Grafana provides a single interface for exploring all three signals.

To turn on monitoring, use the following CLI options when running the application:

- `--with-open-telemetry` (or env variable `AMARU_WITH_OPEN_TELEMETRY`) to export OpenTelemetry metrics, logs, and spans
- `--with-json-traces` (or env variable `AMARU_WITH_JSON_TRACES`) to enable JSON traces on stdout

By default, enabling OpenTelemetry exports all three signals. Pass a comma-separated subset of `metrics`, `traces`, and
`logs` to construct and export only those signals. For example:

```console
amaru --with-open-telemetry=traces node run
```

Disabled signal providers are not constructed, so their signal-specific endpoints do not need to be available. An
empty list or an unknown signal causes startup to fail instead of silently enabling other signals.

## Filtering traces

Trace spans and log events can be filtered by target and severity using two environment variables:

- `AMARU_TRACE`: for any event emitted by the OpenTelemetry layer (enabled both by `--with-open-telemetry` and `--with-json-traces`);
- `AMARU_LOG`: for any event emitted to stdout;

> [!TIP]
> Both environment variable are optional.
>
> - When omitted, `AMARU_TRACE` defaults to all **errors** and **amaru** targets at or above the **info** level;
> - When omitted, `AMARU_LOG` defaults to all **errors** and **amaru** targets at or above the **info** level;

### By target

A `target` is a `::`-separated path of identifiers such as `amaru::ledger::block`. One can filter by providing either a full target, or a sub-path prefix. For example, the target `amaru::ledger` will match the following:

- `amaru::ledger::block`
- `amaru::ledger::epoch_transition`
- `amaru::ledger::store`

But it will not match any of the following:

- `amaru::sync`
- `amaru::consensus`

e.g. `AMARU_LOG="amaru::ledger::epoch_transition=info"` will filter out `target` **amaru::ledger::epoch_transition** with level bellow `info`.

For a comprehensive list of available targets, spans, and traces, see [TRACES.md](../docs/TRACES.md).

### By severity

It is also possible to filter events by severity: `error`, `warn`, `info`, `debug`, `trace`, `off`. Severity can be specified either globally (in which case it applies to all events) or for a specific target by specifying the severity after the target using `=`. For example, `amaru::ledger::block=error` will filter out any events below the error severity for the `amaru::ledger::block` target.

### By span

A `span` name can be used as a filter too. Note that any `span` or `event` inside this `span` will be considered, including those not matching the initial `target` (e.g. `pallas` events could match).
For example `amaru[find_intersection]=trace` will filter all `spans` and `events` with the name `find_intersection` plus all children of this event.

### By tag

Spans can carry functional tags, declared with `tags: <name>, ...` in the schema definitions (see `crates/amaru-observability/src/schemas.rs`). Each tag is recorded on the span as a boolean `amaru.tag.<name>` attribute. The tags currently in use are `cpu`, `setup`, `bootstrap`, and `io`.

To select the spans carrying a given tag, match on the attribute value:

```bash
AMARU_LOG='[{amaru.tag.cpu=true}]=trace'
```

Directives can be combined to match several tags:

- `[{amaru.tag.cpu=true}]=trace,[{amaru.tag.io=true}]=trace` selects spans with the `cpu` **or** the `io` tag;
- `[{amaru.tag.cpu=true,amaru.tag.io=true}]=trace` selects spans with **both** tags.

Like span filters, tag filters are scoped: any `span` or `event` created inside a matching span is also considered.

Note that the value match (`=true`) is required: a field-presence directive such as `[{amaru.tag.cpu}]` only restricts *events*, and matches all spans regardless of their tags.

### Combining filters

Filters can be provided as a sequence of `,`-separated values. Right-most filters take precedence. A usual pattern is to first define a global filter and override it with specific target. For example, `error,amaru::ledger::store=debug` will exclude any event below the `error` severity except those targetting `amaru::ledger::store` which will show up to the `debug` severity.

## Setup

From the repository root, start the complete stack with one command:

```bash
docker compose -f monitoring/docker-compose.yml up -d
```

The stack includes:

- **OpenTelemetry Collector** on `localhost:4317` (OTLP/gRPC)
- **Tempo** for spans and span-derived metrics
- **Prometheus** for application and span-derived metrics
- **Loki** for OpenTelemetry logs and their structured metadata
- **Grafana** with all three data sources provisioned and trace-to-log correlation enabled

Open [Grafana](http://localhost) and use **Explore** to query Tempo, Prometheus, or Loki. The backend endpoints are also available directly:

The provisioned [Amaru Overview dashboard](http://localhost/d/amaru-overview/amaru-overview) is the Grafana home page. It refreshes every five seconds and combines node metrics, live logs, and recent traces containing at least ten spans. Click a trace ID to open its complete span waterfall. Use the `service` field at the top when Amaru is started with a different `OTEL_SERVICE_NAME`.

Two more dashboards, [relay mempool](http://localhost/d/amaru-relay-mempool) and [relay consensus performance](http://localhost/d/amaru-relay-consensus-perf), plot several nodes side by side. Their **Service** selector is populated from the `exported_job` values actually present, so they work for any set of node names and default to all of them; the [relay-1 demo](../scripts/demos/relay-1/README.md) is one producer, not a requirement. Telling nodes apart needs `service.name`, which is why the collector keeps it as the `exported_job` metric label and drops only `service.instance.id`.

- `http://localhost:3200` - Tempo
- `http://localhost:9090` - Prometheus
- `http://localhost:3100` - Loki
- `http://localhost:8889/metrics` - collector's Prometheus scrape endpoint

The Docker volumes retain all three signals across restarts. To stop the stack, run `docker compose -f monitoring/docker-compose.yml down`; add `--volumes` only when the stored telemetry should also be deleted.

## Spans

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

Application metrics are exported by the collector at `http://localhost:8889/metrics` and scraped into Prometheus. Tempo also writes service graph and span metrics to Prometheus, so traces and metrics can be correlated in Grafana.

> [!NOTE]
> The plan so far is to maximise compatibility with the existing Haskell node Prometheus metrics such that tools like [`gLiveView`](https://cardano-community.github.io/guild-operators/Scripts/gliveview/?h=gliveview) and [`nview`](https://github.com/blinklabs-io/nview) keep working out-of-the-box.
>
> We are planning, however, to add more metrics to Amaru.

## Configuring OpenTelemetry

Amaru recognizes the following environment variables for its OpenTelemetry configuration:

- `AMARU_WITH_OPEN_TELEMETRY`: Set to `true` to export all signals, `false` to disable OpenTelemetry, or a comma-separated
  subset of `metrics`, `traces`, and `logs` to export only those signals.
- `OTEL_SERVICE_NAME`: Sets the [service.name](https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/#service-name) key used to identify metrics, logs, and spans. Defaults to `amaru`.
- `OTEL_SERVICE_INSTANCE_ID`: Sets the [service.instance.id](https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/#service-instance-id) key used to identify this specific amaru instance
- `OTEL_EXPORTER_OTLP_ENDPOINT`: Sets the OTLP/gRPC endpoint used to send metrics, logs, and spans. Defaults to `http://localhost:4317`

If you override signal-specific OTLP endpoint variables such as `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`,
`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`, or `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, they should also point to the same
OTLP/gRPC endpoint and should not include a `/v1/...` suffix.

One can find more available env variables [here](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/) and [here](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md).
