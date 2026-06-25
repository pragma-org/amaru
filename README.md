<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://amaru.global/_astro/logo-dark.De0RyNtz.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://amaru.global/_astro/logo-light.C5lipD4m.svg">
    <img alt="Amaru" src="https://amaru.global/_astro/logo-dark.De0RyNtz.svg" height="100">
  </picture>
  <hr />
    <h2 align="center" style="border-bottom: none">A Cardano node client written in Rust.</h2>

[![Licence](https://img.shields.io/github/license/pragma-org/amaru?style=for-the-badge)](https://github.com/pragma-org/amaru/blob/main/LICENSE)
[![Twitter/X](https://img.shields.io/badge/Follow-@amaru__cardano-blue?style=for-the-badge&logo=x)](https://x.com/amaru_cardano)
[![Discord](https://img.shields.io/badge/PRAGMA-%23amaru-5865f2?style=for-the-badge&logo=discord)](https://discord.gg/3nZYCHW9Ns)

  <hr/>
</div>


## Getting Started

> [!WARNING]
>
> Amaru is still in an exploratory phase. Our development strategy favors early
> integration of components, so that progress is instantly visible, even though
> features might be limited or incomplete.

### Installing

<!-- BEGIN GENERATED INSTALLATION -->
#### Docker Image

```console
docker pull ghcr.io/pragma-org/amaru:v10.10.20260625
```

> [!IMPORTANT]
> The tag `:latest` refers to the latest _nightly build_; not the latest release.

#### Homebrew (macOS & Linux)

```console
brew tap pragma-org/amaru https://github.com/pragma-org/amaru && brew trust --tap pragma-org/amaru
brew install amaru
```

#### Nix (macOS & Linux)

```console
nix profile install --no-write-lock-file github:pragma-org/amaru#amaru
```

#### Debian

```console
VERSION=10.10.20260625 ARCH=x86_64; curl -fsSL -o amaru-$VERSION-linux-$ARCH.deb "https://github.com/pragma-org/amaru/releases/download/v$VERSION/amaru-$VERSION-linux-$ARCH.deb"
VERSION=10.10.20260625 ARCH=x86_64; sudo apt install amaru-$VERSION-linux-$ARCH.deb
```

Also available for `ARCH=aarch64`.
The Debian package installs a systemd unit and reads overrides from `/etc/default/amaru`.

#### RPM

```console
VERSION=10.10.20260625 ARCH=x86_64; curl -fsSL -o amaru-$VERSION-linux-$ARCH.rpm "https://github.com/pragma-org/amaru/releases/download/v$VERSION/amaru-$VERSION-linux-$ARCH.rpm"
VERSION=10.10.20260625 ARCH=x86_64; sudo dnf install amaru-$VERSION-linux-$ARCH.rpm
```

Also available for `ARCH=aarch64`.
The RPM package installs a systemd unit and reads overrides from `/etc/sysconfig/amaru`.
<!-- END GENERATED INSTALLATION -->

#### Manual installation: pre-compiled executables

You can install Amaru "manually" by downloading an archive with pre-compiled
(statically linked) executables for all usual platforms (Linux, macOS &
Windows). The archives come with shell completions scripts.

See either:

- [latest releases](https://github.com/pragma-org/amaru/releases);
- [nightly builds](https://pragma-org.github.io/amaru/).


#### Building from sources

```console
make build
```

> [!TIP]
> **Prefer not to install Rust locally?** We provide a Docker-based build and run path.
> See [docker/README.md](./docker/README.md) for instructions on using Docker instead.

### Running

> [!IMPORTANT]
> These instructions assume one starts from scratch, and has access to a synced [cardano-node](https://github.com/IntersectMBO/cardano-node/)
on the selected network (e.g. [preprod](https://book.world.dev.cardano.org/env-preprod.html)).
>
> Although you may explicitly provide peers, Amaru will automatically infer some peers from the ledger state. To run a local peer, refer to [Cardano's developers portal](https://developers.cardano.org/docs/get-started/cardano-node/running-cardano).

1. Bootstrap the node:

```bash
make AMARU_NETWORK=preprod bootstrap
```

2. _(Optional)_ Setup observability backends:

```console
docker-compose -f monitoring/jaeger/docker-compose.yml up
```

3. Run Amaru:

```console
make AMARU_NETWORK=preprod start
```

> [!TIP]
> To ensure logs are forwarded to an OpenTelemetry backend, set `AMARU_WITH_OPEN_TELEMETRY=true`:
>
> ```console
> make AMARU_NETWORK=preprod AMARU_WITH_OPEN_TELEMETRY=true start
> ```

### Monitoring

See [monitoring/README.md](./monitoring/README.md).

<hr/>

<p align="center">
  :boat: <a href="https://github.com/orgs/pragma-org/projects/3">Roadmap</a>
  |
  :triangular_ruler: <a href="./CONTRIBUTING.md">Contributing</a>
  |
  📰 <a href="./CHANGELOG.md">ChangeLog</a>
</p>
