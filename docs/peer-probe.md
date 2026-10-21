# Peer probe

`amaru peer probe` is a one-shot node-to-node (N2N) diagnostic against one or more live peers. It does not run a full node. Use it to check TCP reachability, handshake, tip, peer-share, and keep-alive RTT from an operator machine.

```console
amaru peer probe <TARGETS> [OPTIONS]
```

Put targets before options. Optional-value flags use `=` (for example `--ping=5`), so a following address is not taken as a flag value.

Run `amaru peer probe --help` for the full flag list.

## Targets

`<TARGETS>` is a comma-separated list. Each entry is probed in order:

| Form | Meaning |
| --- | --- |
| `IP:PORT` | Dial that socket |
| `HOSTNAME:PORT` | A/AAAA lookup, then dial |
| bare DNS name | [CIP-0155](https://cips.cardano.org/cip/CIP-0155) SRV lookup of `_cardano._tcp.<name>`; every usable SRV record is resolved and probed (RFC 2782 priority order) |

Examples:

```console
amaru peer probe 1.2.3.4:3001
amaru peer probe relay.example:3001 --ping
amaru peer probe pool.example --all
amaru peer probe 1.2.3.4:3001,relay.example:3001,pool.example --tip
```

## Probe types

With no type flags, the default is ping, handshake, tip, and peer-share (same as `--all` without `--peer-check`).

| Flag | Effect |
| --- | --- |
| `--all` | Run ping, handshake, tip, and peer-share |
| `--ping[=COUNT\|COUNT:INTERVAL]` | Keep-alive RTT after connect + handshake. Bare `--ping` once; `--ping=20:5s` is 20 samples, 5s apart. Human mode prints each sample as it arrives |
| `--handshake` | Query offered N2N versions (`MsgQueryReply`) |
| `--tip` | Tip via chain-sync FindIntersect(Origin) |
| `--peer-share` | Request up to 10 shared peer addresses |
| `--peer-check[=TESTS]` | After peer-share, handshake each returned peer on its own session. Bare `--peer-check` stops after Accept. Extras: `--peer-check=tip;ping` (also `peer-share`). One discovery level only |

## Probe settings

| Flag | Default | Notes |
| --- | --- | --- |
| `--network` | `mainnet` | Handshake network magic (`AMARU_NETWORK`) |
| `--requested-version` | current Amaru N2N (15) | Highest version offered; lower Accept is success |
| `--retry[=COUNT\|COUNT:INTERVAL]` | off | Retries on connect failure, step timeouts, and wall-clock timeout; each attempt gets a fresh budget |
| `--timeout` | `5s` | Wall clock per attempt, from TCP connect until the session finishes (`AMARU_PROBE_TIMEOUT`) |
| `--timeout-connect` | `5s` | TCP connect only |
| `--timeout-handshake` | `10s` | Handshake step |
| `--timeout-protocol` | `15s` | Tip / peer-share / keep-alive reply |
| `--time-track` | off | Per-step timestamps, deltas, and TCP byte counters |
| `--json` | off | Machine-readable JSON (one object, or an array for several targets) |

Global Amaru flags such as `--log-output`, `--with-json-traces`, and `--with-open-telemetry` also apply; see `amaru --help`.

## Output

Human mode streams sections as they complete (including live ping lines). JSON mode emits a single document at the end.

A failed probe exits non-zero when the report contains errors (for example connect failure or timeout after retries).
