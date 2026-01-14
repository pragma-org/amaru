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

```console
make build
```

### Running

> [!IMPORTANT]
> These instructions assume one starts from scratch, and has access to a synced [cardano-node](https://github.com/IntersectMBO/cardano-node/)
on the selected network (e.g. [preprod](https://book.world.dev.cardano.org/env-preprod.html)).
>
> To run a local peer, refer to [Cardano's developers portal](https://developers.cardano.org/docs/get-started/cardano-node/running-cardano).
> Make sure your peer listens to port `3001` or adapt the `AMARU_PEER_ADDRESS` environment variable (e.g. `export AMARU_PEER_ADDRESS=127.0.0.1:3002`)

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

Replace `--peer-address` with your Cardano node peer address. It can be either
a local or remote node (i.e. any existing node relay), and you can even add
multiple peers by replicating the option.

> [!TIP]
> To ensure logs are forwarded to telemetry backend, set `AMARU_WITH_OPEN_TELEMETRY=true`:
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
