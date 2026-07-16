# Monitoring

This document summarizes the various details regarding to monitoring Amaru. As a pre-requisite, it's important to note that Amaru leverages [OpenTelemetry](https://opentelemetry.io/) to emit traces & metrics. A compatible observability backend such as [Jaeger](https://www.jaegertracing.io/), [Grafana Tempo](https://grafana.com/docs/tempo/latest/) and/or [Prometheus](https://prometheus.io/) is therefore needed to collect and visualise telemetry.

We provide example configurations using different compositions of tools:

- Profiles available: Prometheus, Grafana+Tempo, and Jaeger (all optional)

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

The monitoring setup uses Docker Compose with an OTLP collector as the base service. Profile-specific configuration files are located in the `profiles/` subdirectory.

Run with:

```bash
cd monitoring
docker compose up
```

Once running, metrics are available at `http://localhost:8889/metrics` (Prometheus-compatible scrape endpoint).

### Optional Profiles

Prometheus and Grafana+Tempo are available as optional top-level profiles.
Jaeger is provided as a dedicated standalone stack under `profiles/jaeger/` so
that it can keep its own OTLP collector in front of the Jaeger backend.

#### Prometheus

To start Prometheus with the OTLP collector configured to export metrics:

```bash
docker compose -f docker-compose.yml -f profiles/prometheus/docker-compose.yml --profile prometheus up
```

**Includes:**
- **Prometheus** with OTLP collector metrics scrape configuration
- OTLP collector with Prometheus exporter

**Available URLs:**
- `http://localhost:8889/metrics` - OTLP collector metrics endpoint
- `http://localhost:9090` - Prometheus UI

> [!IMPORTANT]
> The extra `-f profiles/prometheus/docker-compose.yml` is required.
> Running only `docker compose --profile prometheus up` starts Prometheus, but
> leaves the OTLP collector on its base config, so metrics are not exported to
> Prometheus.

#### Jaeger

For distributed tracing with Jaeger:

```bash
docker compose -f profiles/jaeger/docker-compose.yml up
```

**Includes:**
- **Jaeger** UI for trace visualization
- **Prometheus** for span metrics
- In-memory span and metrics storage
- An OTLP collector that receives traces, metrics, and logs from Amaru
- Forwarding from the collector to Jaeger for traces and to Prometheus for metrics

**Available URLs:**
- `http://localhost:16686` - Jaeger UI
- `http://localhost:9090` - Prometheus UI
- `http://localhost:8889/metrics` - OTLP collector metrics endpoint

This stack is self-contained and should be started directly, rather than being
combined with the top-level `monitoring/docker-compose.yml`.

#### Grafana

For a visualization dashboard with datasource support:

```bash
docker compose --profile grafana up
```

**Includes:**
- **Grafana** with anonymous access and automatic datasource setup
- Datasource provisioning for Prometheus and Tempo (if running)

**Available URLs:**
- `http://localhost:80` - Grafana UI

#### Tempo

For distributed trace backend storage and visualization:

```bash
docker compose -f docker-compose.yml -f profiles/tempo/docker-compose.yml --profile tempo up
```

**Includes:**
- **Tempo** trace backend with local storage
- OTLP collector with Tempo trace exporter
- Service map integration for topology visualization

**Available URLs:**
- `http://localhost:8889/metrics` - OTLP collector metrics endpoint
- `http://localhost:3200/api/traces` - Tempo traces API

> [!IMPORTANT]
> The extra `-f profiles/tempo/docker-compose.yml` is required.
> Running only `docker compose --profile tempo up` starts Tempo, but leaves the
> OTLP collector on its base config, so traces are not forwarded to Tempo.

#### Combining Profiles: Grafana + Tempo

For the full visualization stack with Grafana and Tempo:

```bash
docker compose -f docker-compose.yml -f profiles/tempo/docker-compose.yml --profile grafana --profile tempo up
```

This enables Grafana to query Tempo traces through the "Explore" → "Tempo" datasource.

#### Combining Profiles

You can combine multiple profiles for different setups:

```bash
# Prometheus metrics only
docker compose -f docker-compose.yml -f profiles/prometheus/docker-compose.yml --profile prometheus up

# Grafana with Prometheus metrics
docker compose -f docker-compose.yml -f profiles/prometheus/docker-compose.yml --profile prometheus --profile grafana up

# Grafana with Tempo traces
docker compose -f docker-compose.yml -f profiles/tempo/docker-compose.yml --profile grafana --profile tempo up

# Full stack: Prometheus, Grafana, and Tempo
docker compose -f docker-compose.yml -f profiles/prometheus/docker-compose.yml -f profiles/tempo/docker-compose.yml --profile prometheus --profile grafana --profile tempo up
```

> [!WARNING]
> When several `-f` files define the `otlp-collector` service, Docker Compose merges their `volumes`
> but **replaces** `command` with the one from the last file. Each `--config` flag must therefore be
> listed in the command of the last profile file: `profiles/tempo/docker-compose.yml` mounts and loads
> the prometheus collector configuration in addition to its own, so the full Prometheus + Grafana +
> Tempo stack keeps its metrics pipeline when the tempo file is listed last.

### Forwarding traces and logs to another OTLP backend

By default, traces and logs are only printed to the collector's console output. To bridge them to an external OTLP-compatible backend (e.g. Jaeger, Grafana Tempo, a remote collector), use the `docker-compose.forward.yml` override:

```bash
# default downstream endpoint (localhost:4319)
docker compose -f docker-compose.yml -f docker-compose.forward.yml up

# custom downstream endpoint
OTLP_FORWARD_ENDPOINT=myhost:4317 docker compose -f docker-compose.yml -f docker-compose.forward.yml up
```

`OTLP_FORWARD_ENDPOINT` controls where traces and logs are forwarded (gRPC, no TLS). It defaults to `host.docker.internal:4319` when not set, which resolves to the host machine from inside the collector container.

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

Coming soon.

> [!NOTE]
> The plan so far is to maximise compatibility with the existing Haskell node Prometheus metrics such that tools like [`gLiveView`](https://cardano-community.github.io/guild-operators/Scripts/gliveview/?h=gliveview) and [`nview`](https://github.com/blinklabs-io/nview) keep working out-of-the-box.
>
> We are planning, however, to add more metrics to Amaru.

## Configuring OpenTelemetry

Amaru recognizes standard OpenTelemetry env variable for its configuration:

- `OTEL_SERVICE_NAME`: Sets the [service.name](https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/#service-name) key used to identify metrics and traces. Defaults to `amaru`.
- `OTEL_SERVICE_INSTANCE_ID`: Sets the [service.instance.id](https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/#service-instance-id) key used to identify this specific amaru instance
- `OTEL_EXPORTER_OTLP_ENDPOINT`: Sets the endpoint used to send spans, defaults to `http://localhost:4317`
- `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`: Sets the endpoint used to send metrics, defaults to `http://localhost:4318/v1/metrics`

Note that two different transports are used internally:

- OTLP/gRPC for spans
- OTLP/HTTP for metrics

This helps maximize compatibility with 3rd party tools receiving those data.

One can find more available env variables [here](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/) and [here](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md).
