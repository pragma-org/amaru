---
id: amaru-advanced-installation
title: "Running an Amaru Node on Mainnet: Advanced Installation"
sidebar_label: "Advanced Installation"
description: Installing Amaru as a systemd service under its own system user, configuring it for mainnet, and operating and maintaining it long-term.
---

This page covers installing the binary, running it as a systemd service under its own dedicated system user, and the ongoing operation and maintenance of that deployment. It assumes the host has already been hardened, see [System Hardening](04-amaru-advanced-hardening.md) first if you haven't done that yet.

## 1. Installing Amaru as a systemd service

:::info version reference
Amaru releases follow a calendar-based scheme (e.g. `10.11.20260807`). Always check the [releases page](https://github.com/pragma-org/amaru/releases) for the latest version before installing. Amaru is still in an exploratory phase, so expect frequent releases and occasional breaking changes.
:::

For production use, run amaru under systemd so it restarts automatically on failure or reboot.
The **Debian / RPM package** is the recommended path as it installs a dedicated amaru system user and the `amaru.service` unit for you.

<details>
<summary><strong>Debian / RPM package</strong> — recommended for systemd deployments</summary>

**Debian/Ubuntu:**
```bash
VERSION=<latest-release> ARCH=x86_64  # check releases page for latest; also available for aarch64
curl -fsSL -o amaru-$VERSION-linux-$ARCH.deb "https://github.com/pragma-org/amaru/releases/download/v$VERSION/amaru-$VERSION-linux-$ARCH.deb"
sudo apt install ./amaru-$VERSION-linux-$ARCH.deb
```

**Fedora/RHEL/CentOS:**
```bash
VERSION=<latest-release> ARCH=x86_64  # check releases page for latest; also available for aarch64
curl -fsSL -o amaru-$VERSION-linux-$ARCH.rpm "https://github.com/pragma-org/amaru/releases/download/v$VERSION/amaru-$VERSION-linux-$ARCH.rpm"
sudo dnf install ./amaru-$VERSION-linux-$ARCH.rpm
```

</details>

## Running Amaru as a systemd service {#running-amaru-as-a-systemd-service}

The Debian and RPM packages don't just ship the amaru binary, they also install a dedicated `amaru` [system user](https://github.com/pragma-org/amaru/blob/c64b7e0444c96264e551e0abc9d1e45c4fb6a710/.github/debian/postinst), a [systemd unit](https://github.com/pragma-org/amaru/blob/c64b7e0444c96264e551e0abc9d1e45c4fb6a710/.github/debian/amaru.service), and a default [environment file](https://github.com/pragma-org/amaru/blob/c64b7e0444c96264e551e0abc9d1e45c4fb6a710/.github/debian/amaru.env).

:::warning TUI Limitation
The use of the TUI is currently only possible when running Amaru directly in a CLI.  
-> A way of using the TUI as a detached process is being explored by the team.  
Please refer to #Monitoring section to learn how to retrieve logs and metrics when Amaru is running as a service.
:::

### The systemd unit (Debian)

- `amaru` user's home is under `/var/lib/amaru` with `nologin`
- `amaru.service` is installed at `/lib/systemd/system/amaru.service` but **not enabled or started**.
- `amaru.env` is installed as `/etc/default/amaru`

```bash title="/etc/default/amaru"
AMARU_NETWORK=mainnet
AMARU_CHAIN_DIR=/var/lib/amaru/chain.mainnet.db
AMARU_LEDGER_DIR=/var/lib/amaru/ledger.mainnet.db
AMARU_PEER_ADDRESS=backbone.mainnet.cardanofoundation.org:3001
AMARU_MIGRATE_CHAIN_DB=true
AMARU_PID_FILE=/run/amaru/amaru.pid
```

## 2. Bootstrapping the node

Assuming you are using the default db location:
```bash
cd /var/lib/amaru
```
:::note
Because the `/var/lib/amaru` folder is owned by the user `amaru` you need to run commands using `sudo -u amaru`.  
Also the Amaru user is set to `nologin` for security concerns.
:::

Amaru bootstraps itself with Mithril-derived snapshots:
```bash
sudo -u amaru amaru node bootstrap --network mainnet
```

This downloads a window of pre-generated, Mithril-derived ledger snapshots from Amaru's snapshot bucket and imports them directly into the chain and ledger databases.
`amaru node run` then only needs to sync the blocks produced since then.
```bash
/var/lib/amaru/
├── chain.mainnet.db/
├── ledger.mainnet.db/
└── snapshots/
```

:::note
Bootstrap fails if the target directories (chain or ledger) already contain data, remove them first before re-bootstrapping:
`amaru node rm --wipe-all-dbs --network mainnet` or `rm -rf chain.mainnet.db/ ledger.mainnet.db/`
:::
If you want to start from a specific epoch instead of the latest available snapshot:

```bash
sudo -u amaru amaru node bootstrap --network mainnet --epoch 602
```
:::info
If the desired epoch snapshot is not available, the cli will respond by listing the usable epoch snapshot for the network.
:::

## 3. Configuring the node

You can override the defaults in `/etc/default/amaru` or add new one as needed.  
Every `amaru node run` [flag](01-amaru-fast-forward.md#4-running-the-node) has a matching `AMARU_*` environment variable, for example:

```bash title="/etc/default/amaru"
[...]
AMARU_PEER_ADDRESS=backbone.mainnet.cardanofoundation.org:3001,my-own-peer.example.com:3001
AMARU_LISTEN_ADDRESS=0.0.0.0:3001
AMARU_UPSTREAM_PEERS=10
AMARU_WITH_OPEN_TELEMETRY=true
OTEL_METRIC_EXPORT_INTERVAL=1000
```

:::note
`AMARU_WITH_OPEN_TELEMETRY` allow Amaru to send traces to the OTLP collector  
`OTEL_METRIC_EXPORT_INTERVAL` is the time between two OTLP metrics export (default is [60s](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/#periodic-exporting-metricreader), here set to 1s) 
:::

You can get the full config list with the TUI under the config menu (pressing `ESC` enter copy mode, enabling cursor selection): 

![Amuru_config_TUI.png](img/Amuru_Config_TUI.png)

<details>
<summary><strong>Here is the same config list in txt</strong></summary>
```bash
┌─ [ AMARU ] [ CARDANO ] [ CONFIG ] ─────────────────────────────────────────────────────────────────────────────────  COPY MODE  ────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│┌─ Essential ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐┌─ Protocol Parameters · Network ─────────────────────────────────────────────────────────────┐│
││Option                       Env                                    Value                                                                       ││Parameter                    Value                                                           ││
││--network                    AMARU_NETWORK                          mainnet                                                                     ││max block body size          90112                                                           ││
││--chain-dir                  AMARU_CHAIN_DIR                        ./chain.mainnet.db                                                          ││max transaction size         16384                                                           ││
││--migrate-chain-db           AMARU_MIGRATE_CHAIN_DB                 false                                                                       ││max block header size        1100                                                            ││
││--ledger-dir                 AMARU_LEDGER_DIR                       ./ledger.mainnet.db                                                         ││max tx ex units              {mem=16500000, cpu=10000000000}                                 ││
││--listen-address             AMARU_LISTEN_ADDRESS                   0.0.0.0:3000                                                                ││max block ex units           {mem=72000000, cpu=20000000000}                                 ││
││--submit-api-address         AMARU_SUBMIT_API_ADDRESS               disabled                                                                    ││max value size               5000                                                            ││
││--peer-address               AMARU_PEER_ADDRESS                     backbone.cardano.iog.io:3001                                                ││max collateral inputs        3                                                               ││
││--peer-snapshot              AMARU_PEER_SNAPSHOT                    none                                                                        │└─────────────────────────────────────────────────────────────────────────────────────────────┘│
│└────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘┌─ Protocol Parameters · Economic ────────────────────────────────────────────────────────────┐│
│┌─ TUI ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐│Parameter                    Value                                                           ││
││Option                       Env                                    Value                                                                       ││min fee a                    44                                                              ││
││--no-tui                     AMARU_NO_TUI                           false                                                                       ││min fee b                    155381                                                          ││
│└────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘│stake credential deposit     2000000                                                         ││
│┌─ Advanced Options ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐│stake pool deposit           500000000                                                       ││
││Option                       Env                                    Value                                                                       ││monetary expansion           3/1000                                                          ││
││--upstream-peers             AMARU_UPSTREAM_PEERS                   3                                                                           ││treasury expansion           2/10                                                            ││
││--downstream-peers           AMARU_DOWNSTREAM_PEERS                 10                                                                          ││min pool cost                170000000                                                       ││
││--max-extra-ledger-snapshots AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS       0                                                                           ││lovelace per UTxO byte       4310                                                            ││
││--peer-removal-cooldown-secs AMARU_PEER_REMOVAL_COOLDOWN_SECS       600                                                                         ││prices                       {mem=577/10000, cpu=721/10000000}                               ││
││--peer-mix                   AMARU_PEER_MIX                         static!2@15m, shared~6, snapshot~3@1h, ledger~3@24h                         ││collateral percentage        150                                                             ││
││--pid-file                   AMARU_PID_FILE                         disabled                                                                    ││ref script fee per byte      15/1                                                            ││
││--trace-buffer               AMARU_TRACE_BUFFER                     disabled                                                                    ││max ref script size per tx   204800                                                          ││
││--dump-trace-buffer          AMARU_DUMP_TRACE_BUFFER                disabled                                                                    ││max ref script size per bloc 1048576                                                         ││
│└────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘│ref script stride            25600                                                           ││
│┌─ Network Global Parameters Overrides ──────────────────────────────────────────────────────────────────────────────────────────────────────────┐│ref script multiplier        12/10                                                           ││
││Option                       Env                                    Value                                                                       │└─────────────────────────────────────────────────────────────────────────────────────────────┘│
││--era-history                AMARU_ERA_HISTORY                      mainnet                                                                     │┌─ Protocol Parameters · Governance ──────────────────────────────────────────────────────────┐│
││--consensus-security-param   AMARU_GLOBAL_CONSENSUS_SECURITY_PARAM  2160                                                                        ││Parameter                    Value                                                           ││
││--epoch-length-scale-factor  AMARU_GLOBAL_EPOCH_LENGTH_SCALE_FACTOR 10                                                                          ││pool max retirement epoch    18                                                              ││
││--active-slot-coeff-inverse  AMARU_GLOBAL_ACTIVE_SLOT_COEFF_INVERSE 20                                                                          ││optimal stake pools          500                                                             ││
││--max-lovelace-supply        AMARU_GLOBAL_MAX_LOVELACE_SUPPLY       45000000000000000                                                           ││pledge influence             3/10                                                            ││
││--slots-per-kes-period       AMARU_GLOBAL_SLOTS_PER_KES_PERIOD      129600                                                                      ││min committee size           7                                                               ││
││--max-kes-evolution          AMARU_GLOBAL_MAX_KES_EVOLUTION         62                                                                          ││max committee term length    146                                                             ││
││--system-start               AMARU_GLOBAL_SYSTEM_START              1506203091000                                                               ││gov action lifetime          6                                                               ││
│└────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘│gov action deposit           100000000000                                                    ││
│                                                                                                                                                  │drep deposit                 500000000                                                       ││
│                                                                                                                                                  │drep expiry                  20                                                              ││
│                                                                                                                                                  └─────────────────────────────────────────────────────────────────────────────────────────────┘│
│                                                                                                                                                                                                                                                 │
└───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── <esc> NORMAL MODE ─┘
```


</details>

## 4. Running the node as a systemd service

The `amaru.service` is not enabled by default, enable it so it can restart on failure or reboot :

```bash
sudo systemctl enable amaru.service
sudo systemctl start amaru
```

You can then read Amaru logs with `journalctl -fu amaru`

## 5. Logs, Metrics and Monitoring

Amaru uses OpenTelemetry to export its Metrics, Logs and Traces. 

When running Amaru in CLI use `--with-open-telemetry`: 

```bash
amaru --with-open-telemetry node run --network preprod 
```

When running Amaru as a systemd service set the env variable `AMARU_WITH_OPEN_TELEMETRY`:
```bash title="/etc/default/amaru"
AMARU_WITH_OPEN_TELEMETRY=true
OTEL_METRIC_EXPORT_INTERVAL=1000
```

:::note
`OTEL_METRIC_EXPORT_INTERVAL` is the time between two OTLP metrics export (default is [60s](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/#periodic-exporting-metricreader), here set to 1s)
:::

A complete Monitoring stack is available in the [Amaru repository](https://github.com/pragma-org/amaru/tree/main/monitoring)

After cloning the repo, run :

```bash
docker compose -f monitoring/docker-compose.yml up -d
```

The stack includes:
- OpenTelemetry Collector on localhost:4317 (OTLP/gRPC)
- Tempo for spans and span-derived metrics
- Prometheus for application and span-derived metrics
- Loki for OpenTelemetry logs and their structured metadata
- Grafana with all three data sources provisioned and trace-to-log correlation enabled

See all details in the [Amaru Monitoring Readme](https://github.com/pragma-org/amaru/tree/main/monitoring#readme)

:::note
If you already have a running Monitoring Stack, feel free to setup the otlp collector to your needs. 

For example if you are only interested in Prometheus metrics to scrape : 

<details>
<summary>docker-compose.yml</summary>
```bash
services:
  otlp-collector:
    image: otel/opentelemetry-collector-contrib:0.133.0
    restart: unless-stopped
    volumes:
      - ./otlp-collector.yml:/etc/otlp-collector.yml
    command: ["--config", "/etc/otlp-collector.yml"]
    ports:
      - "127.0.0.1:4317:4317"
      - "127.0.0.1:8889:8889"
```
</details>

<details>
<summary>otlp-collector.yml</summary>
```bash
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: "0.0.0.0:4317"

processors:
  batch:
  resource/drop_prometheus_labels:
    attributes:
      - key: service.instance.id
        action: delete
  transform/drop_scope:
    metric_statements:
      - context: scope
        statements:
          - set(name, "")
          - set(version, "")

exporters:
  prometheus:
    endpoint: "0.0.0.0:8889"
    add_metric_suffixes: false
    metric_expiration: 60s
  nop:

service:
  pipelines:
    metrics:
      receivers: [otlp]
      processors: [resource/drop_prometheus_labels, transform/drop_scope, batch]
      exporters: [prometheus]
    traces:
      receivers: [otlp]
      exporters: [nop]
    logs:
      receivers: [otlp]
      exporters: [nop]
```
Using the nop exporter allows otlp to receive logs and traces from Amaru and discard them after.

</details>

You can then scrape the metrics at `127.0.0.1:8889/metrics`  
```bash
$ watch -n1 "curl -s 127.0.0.1:8889/metrics | grep -E 'cardano_node_metrics_epoch_int{|cardano_node_metrics_blockNum_int{|cardano_node_metrics_slotInEpoch_int{' | sed 's/{/ /g' | cut -d' ' -f1,3 | column -t -s' ' "

cardano_node_metrics_blockNum_int     1.3850928e+07
cardano_node_metrics_epoch_int        651
cardano_node_metrics_slotInEpoch_int  153412
```
:::

## 6. Administration & maintenance

### Submitting a transaction

Start the node with `--submit-api-address` or `AMARU_SUBMIT_API_ADDRESS`, then `POST` the CBOR-encoded transaction:

```bash
curl -X POST \
  --header "Content-Type: application/cbor" \
  --data-binary @tx.cbor \
  http://localhost:8090/api/submit/tx
```

See Amaru [Submit API](https://github.com/pragma-org/amaru/blob/main/docs/SUBMIT_API.md) documentation for additional details.


### Upgrading Amaru

Amaru is still in an exploratory phase, so check the [release notes](https://github.com/pragma-org/amaru/releases) before every upgrade.

- **Release Binaries** — replace the `amaru` binary on your `$PATH` and restart the process.
- **Debian/RPM package** — download and install the new package the same way as the initial install.  
run `systemctl restart amaru.service` afterwards.
- **Docker** — pull the new tag and recreate the container.
- **Homebrew / Nix** — `brew upgrade amaru` / re-run the `nix profile install` command with the new ref.

:::note
The packaged systemd service sets `AMARU_MIGRATE_CHAIN_DB=true` (`/etc/default/amaru`) so Amaru migrates the on-disk chain database schema forward automatically on upgrade rather than requiring a manual wipe and re-bootstrap.
Best practice is to back up `--chain-dir`/`--ledger-dir` before upgrading across a version that mentions ledger or chain database format changes in its release notes, so you can roll back if needed.
:::

:::warning
When using the CLI directly, `AMARU_MIGRATE_CHAIN_DB` defaults to `false`, so if a db migration is needed, Amaru won't start and will ask to use `--migrate-chain-db` in order to perform the upgrade. 
:::

