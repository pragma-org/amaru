# Relay-1 demo in Docker

Runs the same relay-1 demo as the host version, inside a single container and without flox on
the machine that runs it: the image is the demo flox environment exported with
`flox containerize`, plus a published amaru release and the demo scripts. Every tool the demo
uses — process-compose, cardano-cli, jq, curl, ripgrep, the GNU core utilities — comes from
that environment, so the container and the host demo run the exact same pinned toolset.

The demo bootstraps its databases from the public snapshot CDN and follows a public Cardano
relay, so a fresh container needs nothing but network access.

## Build

Building requires flox and docker; running the image only needs docker.

```bash
./build.sh                                # amaru from the latest published release
AMARU_VERSION=10.11.20260807 ./build.sh   # amaru from a specific release
./build.sh --help                         # all configuration variables
```

The amaru binary is downloaded and checksum-verified on the host, by the flox environment's own
tools, before the image build starts; the build itself needs no network access. The image
matches the build host's Linux architecture — `flox containerize` produces an image for that
architecture only, so there is no cross-architecture build to configure.

**The first build takes a while.** `flox containerize` realizes the environment for Linux inside
its own helper container, which means downloading the whole closure — including cardano-cli's
Haskell runtime — into a docker volume named `flox-nix`. That volume persists, so later builds
reuse it and finish in a couple of minutes. `docker volume rm flox-nix` reclaims the space at
the cost of paying that price again.

When the build finishes it prints the ways to run the demo; they are described below.

## Run

In the foreground, with the Process Compose TUI (`F10` shuts the demo down, `F6` toggles log
wrapping, the arrow keys walk the processes):

```bash
docker run -it --name relay-1 -p 8091:8091 -v amaru-relay-1:/data amaru-relay-1:latest
```

Or in the background, attaching the TUI whenever you want to look. Quitting an attached TUI
leaves the demo running, which is usually what you want while observing:

```bash
docker run -d --name relay-1 -p 8091:8091 -v amaru-relay-1:/data amaru-relay-1:latest
docker exec -it relay-1 process-compose attach
```

A fresh container reaches the network tip in a few minutes: about 90 seconds to download and
import the three epoch snapshots, then both relays start and catch up. Restarting a container
that keeps its volume skips the bootstrap entirely.

Useful while it runs:

```bash
docker logs -f relay-1                                              # colorized watch output
curl http://localhost:8091/                                         # the submit API answers
docker exec -it relay-1 process-compose process restart 7-submit-tx  # submit another transaction
docker exec -it relay-1 bash                                        # a shell in the activated env
```

- `/data` holds the bootstrapped databases, the per-node run directories and the logs. Keep the
  volume between runs; the bootstrap is only redone when it is missing.
- Port 8091 is the downstream node's transaction submit API. Ports 4001 and 4002 (the relays'
  peer listen ports) can be published too, if other nodes should connect to them.
- Configure with the same environment variables as the host demo, for instance
  `-e AMARU_NETWORK=preview` or `-e TX_GENERATED_COUNT=5`. See the
  [demos README](../../README.md) for the full list.
- To exercise submission through the middle relay instead of the downstream one, point the
  submitting processes at its API: `-e TX_SUBMIT_API_ADDRESS=127.0.0.1:8090`. Only the node that
  receives a transaction holds it in its mempool — transactions travel from a connection's
  initiator to its server, so a transaction handed to the middle relay never reaches the
  downstream node's mempool, and only the middle relay reports it coming back in a block.

## Telemetry

The container never runs Grafana, Tempo, Prometheus or Loki itself: they need a docker daemon,
which the container does not have. Run the shared monitoring stack on the host and join its
network — that one flag is all the wiring needed.

```bash
(cd ../../../../monitoring && docker compose up -d --remove-orphans)   # Grafana on http://localhost

docker run -d --name relay-1 --network monitoring_default \
  -p 8091:8091 -v amaru-relay-1:/data amaru-relay-1:latest
```

Joining `monitoring_default` lets the container reach the collector by its service name, with no
extra ports published. The image already points both OTLP endpoints at `otlp-collector:4317` (the
collector only enables the gRPC receiver, and amaru exports all three signals over gRPC), and the
demo probes that endpoint at startup: it exports when the collector answers and stays quiet
otherwise, so the same image runs with or without the network.

The stack provisions the relay dashboards, and the demo's exported trace filter already includes the
debug-level consensus spans they query, so the trace panels fill in on their own. If they stay
empty, check `AMARU_DEMO_TRACE`: an `info`-only filter drops those spans while metrics and logs keep
flowing, which makes the wiring look fine when it is the filter that is wrong.

`--remove-orphans` is worth passing once: earlier versions of this demo layered a promtail service
onto the stack, and it lingers otherwise.

## Wipe and re-bootstrap

```bash
docker rm -f relay-1
docker volume rm amaru-relay-1
```

Or keep the volume and bootstrap again once, reusing the snapshot archives already downloaded
into it:

```bash
docker run --rm -v amaru-relay-1:/data amaru-relay-1:latest bash ./process-compose.sh refresh
```
