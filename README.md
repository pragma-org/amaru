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
<div align="center">

[![][ci-build-and-test-badge]][ci-build-and-test]
[![][ci-coding-practices-badge]][ci-coding-practices]
[![][nightly-synchronization-badge]][nightly-synchronization]
[![][nightly-tool-integrations-badge]][nightly-tool-integrations]
[![][nightly-uplc-benchmarks-badge]][nightly-uplc-benchmarks]

</div>

## Getting Started

Amaru is still in an exploratory phase. Our development strategy favors early
integration of components, so that progress is instantly visible, even though
features might be limited or incomplete.

### Installing

#### Manual installation: pre-compiled executables

You can install Amaru "manually" by downloading an archive with pre-compiled
(statically linked) executables for all usual platforms (Linux, macOS &
Windows). The archives come with shell completions scripts.

See either:

- [latest releases](https://github.com/pragma-org/amaru/releases);
- [nightly builds](https://pragma-org.github.io/amaru/).

Alternatively, Amaru also comes pre-packaged for a few platforms.

<!-- BEGIN GENERATED INSTALLATION -->
#### <img src="https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/.github/icons/docker.svg" height=18 /> Docker Image

```console
docker pull ghcr.io/pragma-org/amaru:v10.11.20260903
```

> [!IMPORTANT]
> The tag `:latest` refers to the latest _nightly build_; not the latest release.

#### <img src="https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/.github/icons/homebrew.webp" height=18 /> Homebrew (macOS & Linux)

```console
brew tap pragma-org/amaru https://github.com/pragma-org/amaru && brew trust --tap pragma-org/amaru
brew install amaru
```

#### <img src="https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/.github/icons/nix.webp" height=18 /> Nix (macOS & Linux)

```console
nix profile install --no-write-lock-file github:pragma-org/amaru#amaru
```

#### <img src="https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/.github/icons/debian.webp" height=18 /> Debian

```console
VERSION=10.11.20260903 ARCH=x86_64; curl -fsSL -o amaru-$VERSION-linux-$ARCH.deb "https://github.com/pragma-org/amaru/releases/download/v$VERSION/amaru-$VERSION-linux-$ARCH.deb"
VERSION=10.11.20260903 ARCH=x86_64; sudo apt install ./amaru-$VERSION-linux-$ARCH.deb
```

Also available for `ARCH=aarch64`.
The Debian package installs a systemd unit and reads overrides from `/etc/default/amaru`.

#### <img src="https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/.github/icons/red-hat.webp" height=18 /> RPM

```console
VERSION=10.11.20260903 ARCH=x86_64; curl -fsSL -o amaru-$VERSION-linux-$ARCH.rpm "https://github.com/pragma-org/amaru/releases/download/v$VERSION/amaru-$VERSION-linux-$ARCH.rpm"
VERSION=10.11.20260903 ARCH=x86_64; sudo dnf install ./amaru-$VERSION-linux-$ARCH.rpm
```

Also available for `ARCH=aarch64`.
The RPM package installs a systemd unit and reads overrides from `/etc/sysconfig/amaru`.
<!-- END GENERATED INSTALLATION -->

#### Building from sources

```console
cargo build --release
```

> [!TIP]
> **Prefer not to install Rust locally?** We provide a Docker-based build and run path.
> See [docker/README.md](./docker/README.md) for instructions on using Docker instead.

### Running

1. _(First time)_ Bootstrap the node:

```bash
amaru node bootstrap --network=mainnet
```

2. _(Optional)_ Setup observability backends:

```console
docker compose -f monitoring/docker-compose.yml up -d
```

3. Run Amaru:

```console
amaru node run --network=mainnet
```

> [!TIP]
> To ensure logs are forwarded to an OpenTelemetry backend, set `AMARU_WITH_OPEN_TELEMETRY=true`:
>
> ```console
> amaru --with-open-telemetry node run --network=mainnet
> ```
>
> See also `amaru node run --help` for more options.

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

[ci-build-and-test-badge]: https://img.shields.io/github/actions/workflow/status/pragma-org/amaru/ci-build-and-test.yml?style=flat-square&logo=rust&logoColor=%23ffffff&label=
[ci-build-and-test]: https://github.com/pragma-org/amaru/actions/workflows/ci-build-and-test.yml

[ci-coding-practices-badge]: https://img.shields.io/github/actions/workflow/status/pragma-org/amaru/ci-coding-practices.yml?style=flat-square&logo=thestorygraph&logoColor=%23ffffff&label=
[ci-coding-practices]: https://github.com/pragma-org/amaru/actions/workflows/ci-coding-practices.yml

[nightly-synchronization-badge]: https://img.shields.io/github/actions/workflow/status/pragma-org/amaru/nightly-synchronization.yml?style=flat-square&logo=cardano&logoColor=%23ffffff&label=
[nightly-synchronization]: https://github.com/pragma-org/amaru/actions/workflows/nightly-synchronization.yml

[nightly-tool-integrations-badge]: https://img.shields.io/github/actions/workflow/status/pragma-org/amaru/nightly-tool-integrations.yml?style=flat-square&logo=applearcade&logoColor=%23ffffff&label=
[nightly-tool-integrations]: https://github.com/pragma-org/amaru/actions/workflows/nightly-tool-integrations.yml

[nightly-uplc-benchmarks-badge]: https://img.shields.io/github/actions/workflow/status/pragma-org/amaru/nightly-uplc-benchmarks.yml?style=flat-square&logo=blockbench&logoColor=%23ffffff&label=
[nightly-uplc-benchmarks]: https://github.com/pragma-org/amaru/actions/workflows/nightly-uplc-benchmarks.yml
