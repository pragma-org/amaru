# Monitoring

This document summarizes the various details regarding to monitoring Amaru. As a pre-requisite, it's important to note that Amaru leverages [OpenTelemetry](https://opentelemetry.io/) to emit traces & metrics. A compatible observability backend such as [Jaeger](https://www.jaegertracing.io/), [Grafana Tempo](https://grafana.com/docs/tempo/latest/) and/or [Prometheus](https://prometheus.io/) is therefore needed to collect and visualise telemetry.

We provide example configurations using different compositions of tools:

- [Jaeger + Prometheus](./jaeger) _(simple)_
- [Grafana + Tempo + Prometheus](./grafana-tempo) _(more advanced)_

To turn on monitoring, use the following CLI options when running the application:

* `--with-open-telemetry` (or env variable `AMARU_WITH_OPEN_TELEMETRY`) to enable [OpenTelemetry](https://opentelemetry.io/) traces
* `--with-json-traces` (or env variable `AMARU_WITH_JSON_TRACES`) to enable JSON traces on stdout

## Filtering traces

Any event (trace, span or metric) can be filtered by target and severity using two environment variables:

- `AMARU_TRACE`: for any event emitted by the OpenTelemetry layer (enabled both by `--with-open-telemetry` and `--with-json-traces`);
- `AMARU_LOG`: for any event emitted to stdout;

> [!TIP]
> Both environment variable are optional.
>
> - When omitted, `AMARU_TRACE` defaults to all **amaru** targets above the **trace** level;
> - When omitted, `AMARU_LOG` defaults to all **amaru** targets above the **debug** level;

### By target

A `target` is a `::`-separated path of identifiers such as `amaru::ledger::state`. One can filter by providing either a full target, or a sub-path prefix. For example, the target `amaru::ledger` will match the following:

- `amaru::ledger::state`
- `amaru::ledger::state::forward`
- `amaru::ledger::store`

But it will not match any of the following:

- `amaru::sync`
- `amaru::consensus`

e.g. `AMARU_LOG="amaru::ledger::state::forward=info"` will filter out `target` **amaru::ledger::state::forward** with level bellow `info`.

Refer to the tables below for the list of available targets.

### By severity

It is also possible to filter events by severity: `error`, `warn`, `info`, `debug`, `trace`, `off`. Severity can be specified either globally (in which case it applies to all events) or for a specific target by specifying the severity after the target using `=`. For example, `amaru::ledger::state=error` will filter out any events below the error severity for the `amaru::ledger::state` target.

Refer to the tables below for an overview of the various severities.

### By span

A `span` name can be used as a filter too. Note that any `span`or `event` inside this `span` will be considered, including those not matching the initial `target` (e.g. `pallas` events could match).
For example `amaru[find_intersection]=trace` will filter all `spans` and `events` with the name `find_intersection` plus all children of this event.

### Combining filters

Filters can be provided as a sequence of `,`-separated values. Right-most filters take precedence. A usual pattern is to first define a global filter and override it with specific target. For example, `error,amaru::ledger::store=debug` will exclude any event below the `error` severity except those targetting `amaru::ledger::store` which will show up to the `debug` severity.

## Traces & spans

### target: `amaru::ledger`

#### Traces

ø

#### Spans

| name       | severity | description                             |
| ---        | ---      | ---                                     |
| `forward`  | `info`   | Wraps the processing of a block forward |
| `backward` | `info`   | Wraps the processing of a rollback      |

<details><summary>span: `forward`</summary>

| field               | description                                                      |
| ---                 | ---                                                              |
| `header.height`     | Absolute block height                                            |
| `header.slot`       | Absolute slot number                                             |
| `header.hash`       | Block header hash                                                |
| `stable.epoch`      | Current epoch of the most recent stable point                    |
| `tip.epoch`         | Current epoch of the most recent volatile point                  |
| `tip.relative_slot` | Relative slot within the epoch of the most recent volatile point |
</details>

<details><summary>span: `backward`</summary>

| field        | description                                       |
| ---          | ---                                               |
| `point.slot` | Absolute slot number of the target rollback point |
| `point.hash` | Block header hash of the target rollback point    |
</details>

### target: `amaru::ledger::state`

#### Traces

| name                  | severity | description                                                          |
| ---                   | ---      | ---                                                                  |
| `volatile.warming_up` | `info`   | Emitted on start-up while the ledger volatile database is filling up |

<details><summary>trace: `volatile.warming_up`</summary>

| field  | description                                 |
| ---    | ---                                         |
| `size` | Current size of the volatile db, up to `k`. |
</details>

#### Spans

| name                  | severity | description                                                                           |
| ---                   | ---      | ---                                                                                   |
| `block.body.validate` | `info`   | Wraps the block body validation & processing                                          |
| `snapshot`            | `info`   | Wraps the creation of a new epoch-boundary snapshot                                   |
| `save`                | `info`   | Wraps the persistence on-disk of the next now-stable ledger delta                     |
| `tick.pool`           | `info`   | Wraps the update of pool parameters and enactment of retirements at an epoch-boundary |
| `apply.transaction`   | `info`   | Wraps the validation & processing of a single transaction                             |

<details><summary>span: `block.body.validate`</summary>

| field                        | description                                          |
| ---                          | ---                                                  |
| `block.transactions.total`   | Total number of transactions in the block            |
| `block.transactions.failed`  | Total number of failed transactions in the block     |
| `block.transactions.success` | Total number of successful transactions in the block |
</details>


<details><summary>span: `snapshot`</summary>

| field   | description                                                                    |
| ---     | ---                                                                            |
| `epoch` | The epoch being captured as snapshot, typically the immediately previous epoch |
</details>

### target: `amaru::ledger::state::rewards`

#### Spans

ø

#### Traces

| name                          | severity | description                                                              |
| ---                           | ---      | ---                                                                      |
| `stake_distribution.snapshot` | `info`   | A new stake distribution snapshot has been computed and is now available |
| `rewards.summary`             | `info`   | Rewards have been computed and are now available                         |

<details><summary>trace: `stake_distribution.snapshot`</summary>

| field          | description                                                                |
| ---            | ---                                                                        |
| `active_stake` | Total stake, in Lovelace, delegated to registered pools                    |
| `accounts`     | Total number of *active* (i.e. delegated to active pools) staking accounts |
| `pools`        | Total number of active pools                                               |
</details>

<details><summary>trace: `rewards.summary`</summary>

 | field               | description                                                                                                               |
 | ---                 | ---                                                                                                                       |
 | `efficiency`        | The ratio of total blocks produced in the epoch, over the expected number of blocks (determined by protocol parameters).  |
 | `incentives`        | The total amount taken out of the reserves as incentivies at this particular epoch (a.k.a. ΔR1).                          |
 | `treasury_tax`      | Portion of the rewards going to the treasury (irrespective of unallocated pool rewards).                                  |
 | `total_rewards`     | Total rewards available before the treasury tax. In particular, we have: total_rewards = treasury_tax + available_rewards |
 | `available_rewards` | Remaining rewards available to stake pools (and delegators)                                                               |
 | `effective_rewards` | Effective rewards _actually given out_. The surplus is "sent back" to the reserves.                                       |
 | `pots.reserves`     | Value, in Lovelace, of the reserves after rewards distribution.                                                           |
 | `pots.treasury`     | Value, in Lovelace, of the treasury after rewards distribution.                                                           |
 | `pots.fees`         | Values, in Lovelace, generated from fees during an epoch.                                                                 |
</details>

### target: `amaru::ledger::state::transaction`

#### Spans

<details><summary>span: `apply.transaction`</summary>

| field                      | description                                               |
| ---                        | ---                                                       |
| `transaction.id`           | The transaction identifier/hash                           |
| `transaction.certificates` | The number of certificates within the transaction         |
| `transaction.inputs`       | The number of (collateral) inputs within the transaction  |
| `transaction.outputs`      | The number of (collateral) outputs within the transaction |
| `transaction.withdrawals`  | The number of withdrawals within the transaction          |
</details>

#### Traces

| name                               | severity | description                                          |
| ---                                | ---      | ---                                                  |
| `certificate.stake.registration`   | `trace`  | A new stake credential registration was processed    |
| `certificate.stake.delegation`     | `trace`  | A new stake delegation was processed                 |
| `certificate.stake.deregistration` | `trace`  | A new stake credential de-registration was processed |
| `certificate.pool.registration`    | `trace`  | A new stake pool registration was processed          |
| `certificate.pool.retirement`      | `trace`  | A new stake pool retirement was processed            |
| `certificate.drep.registration`    | `trace`  | A new drep registration was processed                |
| `certificate.drep.unregistration`  | `trace`  | A new drep unregistration was processed              |
| `certificate.drep.update`          | `trace`  | A new drep anchor update was processed               |
| `certificate.vote.delegation`      | `trace`  | A new vote delegation was processed                  |

<details><summary>trace: `certificate.stake.registration`</summary>

| field        | description                       |
| ---          | ---                               |
| `credential` | Stake credential being registered |
</details>

<details><summary>trace: `certificate.stake.delegation`</summary>

| field        | description                      |
| ---          | ---                              |
| `credential` | Stake credential being delegated |
| `pool`       | Stake pool delegate              |
</details>

<details><summary>trace: `certificate.stake.deregistration`</summary>

| field        | description                       |
| ---          | ---                               |
| `credential` | Stake credential being deregistered |
</details>

<details><summary>trace: `certificate.stake.registration`</summary>

| field    | description                          |
| ---      | ---                                  |
| `pool`   | Stake pool identifier                |
| `params` | New or initial stake pool parameters |
</details>

<details><summary>trace: `certificate.stake.retirement`</summary>

| field   | description           |
| ---     | ---                   |
| `pool`  | Stake pool identifier |
| `epoch` | Retirement epoch      |
</details>

<details><summary>trace: `certificate.drep.registration`</summary>

| field    | description                      |
| ---      | ---                              |
| `drep`   | DRep identifier being retired    |
| `refund` | Mandatory Lovelace refund amount |
</details>

<details><summary>trace: `certificate.drep.retirement`</summary>

| field     | description                                  |
| ---       | ---                                          |
| `drep`    | DRep identifier being registered             |
| `deposit` | Mandatory Lovelace deposit amount            |
| `anchor`  | An optional DRep anchor (URL + content hash) |
</details>


<details><summary>trace: `certificate.vote.delegation`</summary>

| field        | description                      |
| ---          | ---                              |
| `credential` | Stake credential being delegated |
| `drep`       | Target drep for delegation       |
</details>

### target: `amaru::ledger::store`

#### Spans

ø

#### Traces

| name                       | severity | description                                                                          |
| ---                        | ---      | ---                                                                                  |
| `new.unexpected_file`      | `warn`   | An unexpected file is present in the database folder, which may cause an issue later |
| `save.point_already_known` | `trace`   | Skipping saving ledger delta because it has already happened                         |
| `new.known_snapshots`      | `info`  | A previous database snapshot has been found in the database folder                   |

<details><summary>trace: `new.unexpected_file`</summary>

| field      | description                                           |
| ---        | ---                                                   |
| `filename` | The relative path to the unexpected file or directory |
</details>

<details><summary>trace: `save.point_already_known`</summary>

| field   | description                                      |
| ---     | ---                                              |
| `point` | The now-stable point already known and persisted |
</details>

<details><summary>trace: `new.known_snapshots`</summary>

| field   | description                                   |
| ---     | ---                                           |
| `epoch` | The snapshot number / epoch it is relevant to |
</details>

### target: `amaru::ledger::store::accounts`

#### Spans

ø

#### Traces

| name                      | severity | description                                                                                             |
| ---                       | ---      | ---                                                                                                     |
| `add.register_no_deposit` | `error`  | Attempting (and failing) to register a brand-new account without deposit; signaling a bug in the ledger |

<details><summary>trace: `add.register_no_deposit`</summary>

| field        | description                            |
| ---          | ---                                    |
| `credential` | Stake credential of the faulty account |
</details>

### target: `amaru::ledger::store::pools`

#### Traces

| name            | severity | description                                             |
| ---             | ---      | ---                                                     |
| `tick.retiring` | `trace`  | Now-retiring (i.e. removing from stable storage) a pool |
| `tick.updating` | `trace`  | Updating a pool's parameters                            |

<details><summary>trace: `tick.retiring`</summary>

| field  | description                                  |
| ---    | ---                                          |
| `pool` | The identifier of the now-retired stake pool |
</details>

<details><summary>trace: `tick.updating`</summary>

| field        | description                                   |
| ---          | ---                                           |
| `pool`       | The identifier of the updated stake pool      |
| `new_params` | The new/now-effective parameters for the pool |
</details>


#### Spans

ø

## Metrics

Coming soon.

> [!NOTE]
> The plan so far is to maximise compatibility with the existing Haskell node Prometheus metrics such that tools like [`gLiveView`](https://cardano-community.github.io/guild-operators/Scripts/gliveview/?h=gliveview) and [`nview`](https://github.com/blinklabs-io/nview) keep working out-of-the-box.
>
> We are planning, however, to add more metrics to Amaru.

## Configuring OpenTelemetry

Amaru provides a few options to configure OpenTelemetry:

- `--otlp-service-name <STRING>` (environment variable `AMARU_OTLP_SERVICE_NAME`): Sets the `service.name` key used to identify metrics and traces. This is useful when a single OTLP service stack collects telemetry from several Amaru instances
- `--otlp-span-url <STRING>` (environment variable `AMARU_OTLP_SPAN_URL`): Sets the endpoint used to send spans, defaults to `http://localhost:4317`
- `--otlp-metric-url <STRING>` (environment variable `AMARU_OTLP_METRIC_URL`): Sets the endpoint used to send metrics, defaults to `http://localhost:4318/v1/metrics`
