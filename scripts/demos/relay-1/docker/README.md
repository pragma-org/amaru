# Relay-1 demo in Docker

Runs the same relay-1 demo as the host version, inside a single container and without flox on
the machine that runs it: the image is the demo flox environment exported with
`flox containerize`, plus published amaru and cardano-cli releases and the demo scripts. Every tool the demo
uses — process-compose, jq, curl, ripgrep, the GNU core utilities — comes from that environment,
so the container and the host demo run the exact same pinned toolset. The two static binaries that
are not flox packages, amaru and cardano-cli, are the same pinned releases the host demo downloads.

The demo bootstraps its databases from the public snapshot CDN and follows a public Cardano
relay, so a fresh container needs nothing but network access.

## Pull

The published image is the shortest path: no checkout, no flox, no Rust toolchain, nothing to
build. Pull the tag for your architecture and run it:

```bash
docker pull ghcr.io/pragma-org/amaru-relay-1-demo:arm64
docker run -it -v amaru-relay-1:/data ghcr.io/pragma-org/amaru-relay-1-demo:arm64
```

Use `:amd64` on an Intel or AMD machine, and `:arm64` on Apple silicon or an ARM server. The tags
are architecture-specific because the image cannot be cross-built (see [Build](#build) below), so
each one is produced on a machine of that architecture.

There is also a `:latest` tag once both architectures have been published together: it is a
manifest list, so `docker pull` picks the right image for the machine and you can stop thinking
about architectures. Prefer it when it is there.

All of these come from the
[Publish relay-1 demo image](../../../../.github/workflows/publish-relay-1-demo-image.yml)
workflow. Build the image yourself instead when you want an unreleased amaru or your own demo
changes.

## Build

Building requires flox and docker; running the image only needs docker.

```bash
./build.sh                                # amaru from the latest published release
AMARU_VERSION=10.11.20260807 ./build.sh   # amaru from a specific release
./build.sh --help                         # all configuration variables
```

The amaru and cardano-cli binaries are downloaded and checksum-verified on the host, by the flox
environment's own tools, before the image build starts; the build itself needs no network access.
Both are statically linked, which the image requires: it has no dynamic loader for anything built
against a distro libc.

Containerizing the flox environment is the slow and fragile step, so its image is tagged with a
digest of `.flox/env/manifest.lock` and reused when that tag already exists. An unchanged environment
therefore rebuilds only the thin layer on top, while any change to the lock produces a tag that does
not exist yet and containerizes again. Pass `BASE_TAG` to point at a base built elsewhere.

The image names `CARDANO_CLI` explicitly, and the build asserts that the container resolves the
pinned `CARDANO_CLI_RELEASE_VERSION`. Both exist because a `cardano-cli` inside the flox environment
would come ahead of `/usr/local/bin` on `PATH` and silently replace the checksum-verified binary.

👉 If `flox containerize` fails with `unable to load seccomp BPF program`, the cached `nixos/nix`
helper image is for the wrong architecture; nix then runs under emulation, where its build sandbox
cannot start. Re-pull it for the host: `docker rmi nixos/nix:<tag> && docker pull --platform
linux/<arch> nixos/nix:<tag>`.

The image always matches the build host's architecture, and there is no cross-build to configure.
`flox containerize` has no architecture flag, and forcing its helper container to another platform
does not work either: nix fails with `unable to load seccomp BPF program` under emulation, and flox
sets `NIX_CONFIG` itself, so the sandbox cannot be turned off from outside. `build.sh` refuses
outright when `DOCKER_DEFAULT_PLATFORM` disagrees with the host, rather than letting that surface
as an exec format error once the container starts.

Both architectures therefore come from native runners, which is what
[the publish workflow](../../../../.github/workflows/publish-relay-1-demo-image.yml) is for.

**The first build takes a while.** `flox containerize` realizes the environment for Linux inside
its own helper container, which means downloading the whole closure into a docker volume named
`flox-nix`. That volume persists, so later builds reuse it and finish in a couple of minutes.
`docker volume rm flox-nix` reclaims the space at the cost of paying that price again.

When the build finishes it prints the ways to run the demo; they are described below.

## Run

In the foreground, with the Process Compose TUI (`F10` shuts the demo down, `F6` toggles log
wrapping, the arrow keys walk the processes):

```bash
docker run -it --name relay-1 -v amaru-relay-1:/data amaru-relay-1:latest
```

Or in the background, attaching the TUI whenever you want to look. Quitting an attached TUI
leaves the demo running, which is usually what you want while observing:

```bash
docker run -d --name relay-1 -v amaru-relay-1:/data amaru-relay-1:latest
docker exec -it relay-1 process-compose attach
```

A fresh container reaches the network tip in a few minutes on preprod or preview: about 90 seconds
to download and import the three epoch snapshots, then both relays start and catch up. Mainnet is
larger and further behind, so budget roughly 5 to 8 minutes for the bootstrap and another 20 or so
for the replay. Restarting a container that keeps its volume skips both entirely.

Useful while it runs:

```bash
docker logs -f relay-1                                              # colorized watch output
docker exec -it relay-1 process-compose process restart 7-submit-tx  # submit another transaction
docker exec -it relay-1 bash                                        # a shell in the activated env
```

- `/data` holds the bootstrapped databases, the per-node run directories and the logs. Keep the
  volume between runs; the bootstrap is only redone when it is missing.
- No ports need publishing. The demo submits its transactions from inside the container, to the
  downstream node's submit API on 8091. Add `-p 8091:8091` to post transactions from the host, and
  `-p 4001:4001 -p 4002:4002` to let other nodes connect to the relays' peer listeners.
- Configure with the same environment variables as the host demo, for instance
  `-e AMARU_NETWORK=preview` or `-e TX_GENERATED_COUNT=5`. See the
  [demos README](../../README.md) for the full list, and
  [Mainnet, with your own wallet](#mainnet-with-your-own-wallet) for a worked example.
- Every generated transaction carries `TX_METADATA_MESSAGE` as a CIP-20 message under metadata label
  674, so a transaction that travelled through the relays is identifiable by its comment in a public
  explorer. Set `-e TX_METADATA_MESSAGE=` to attach none.
- To exercise submission through the middle relay instead of the downstream one, point the
  submitting processes at its API: `-e TX_SUBMIT_API_ADDRESS=127.0.0.1:8090`. Only the node that
  receives a transaction holds it in its mempool — transactions travel from a connection's
  initiator to its server, so a transaction handed to the middle relay never reaches the
  downstream node's mempool, and only the middle relay reports it coming back in a block.

## Mainnet, with your own wallet

The default network is preprod, whose committed key holds faucet ada. On mainnet the demo spends
**real ada** from a key you provide, so this section is worth reading before running it.

Start the monitoring stack, then the demo, with the payment key mounted read-only and a second
upstream peer alongside the public relay:

```bash
docker compose -f ../../../../monitoring/docker-compose.yml up -d --remove-orphans

docker run -d --name relay-1 --network monitoring \
  -e AMARU_NETWORK=mainnet \
  -e TX_REFUEL_UTXO_COUNT=6 \
  -e PUBLIC_UPSTREAM_PEER_ADDRESS="backbone.cardano.iog.io:3001 my-own-relay.example:3001" \
  -v /path/to/addr_xsk:/data/run/mainnet-wallet/payment.skey:ro \
  -v amaru-relay-1-mainnet:/data \
  amaru-relay-1:latest

docker exec -it relay-1 process-compose attach
```

What each part is doing, and why it is written that way:

- **The key needs no environment variable.** `/data/run/mainnet-wallet/payment.skey` is where the demo
  looks by default, because `RUNDIR` is the data volume in the container. A `cardano-address`
  `addr_xsk` file works directly; the demo recognises it and converts it per process. A `root_xsk` is
  rejected, since converting it silently yields an address that never holds the funds.
- **Mount the file, not its directory.** The directory holding an `addr_xsk` usually holds `root_xsk`
  too, and the container has no business seeing a master key.
- **`TX_PAYMENT_SKEY` can carry the key itself** instead of a path, which avoids the mount
  altogether. Convenient for a throwaway key, poor for real funds: the value is readable for the
  container's whole life through `docker inspect`, and lands in shell history too.
- **A separate volume per network.** The bootstrap marker is per-network, so nothing clashes, but
  mainnet databases are large and worth being able to delete on their own.
- **`PUBLIC_UPSTREAM_PEER_ADDRESS` replaces the default** rather than adding to it, so list the
  public relay explicitly when adding one of your own. Watch a new peer connect with
  `docker exec relay-1 rg 'adding peer|removing peer' /data/logs/amaru-middle.log`.

Fund the address the demo derives, which is an **enterprise** address (`addr1v…`) and therefore not
the address a wallet application shows for the same key. It is printed by `6-prepare-wallet` and
`7-submit-tx` as `using payment address:`. Around 10 ada is plenty: each submitted transaction costs
roughly 0.17 ada in fees, and wallet preparation about the same.

To keep more runs out of the same funds, leave `6-prepare-wallet` alone between batches. A submit
drains each input into a slightly smaller output that stays spendable, so several rounds need no
preparation transaction at all, and preparation only happens once too few outputs clear the
threshold. Fewer concurrent replicas also go further, because every idle output strands the protocol
minimum in ada that can never become fees.

## Telemetry

The container never runs Grafana, Tempo, Prometheus or Loki itself: they need a docker daemon,
which the container does not have. Run the shared monitoring stack on the host and join its
network — that one flag is all the wiring needed.

```bash
docker compose -f ../../../../monitoring/docker-compose.yml up -d --remove-orphans   # Grafana on http://localhost

docker run -d --name relay-1 --network monitoring \
  -v amaru-relay-1:/data amaru-relay-1:latest
```

Joining the `monitoring` network lets the container reach the collector by its service name, with no
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
