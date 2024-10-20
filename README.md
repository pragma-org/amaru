# Amaru

Amaru is a [Cardano](https://cardano.org) node client written in Rust. It is an ambitious project which aims to bring more diversity to the infrastructure operating the Cardano network.

## Getting Started

> [!WARNING]
>
> Amaru is still in an exploratory phase. Our development strategy favors early
> integration of components, so that progress is instantly visible, even though
> features might be limited or incomplete.

### Installing

```console
cargo build --release
```

### Running (demo)

```console
RUST_LOG=info cargo run --release -- daemon \
  --peer-address=127.0.0.1:3000 \
  --network=preprod \
  --from=72662420.dcda918eca2dbb3b574ca9256435b20c4457a80a438d757d2774689e602d1110
```

> [!TIP]
> Replace `--peer-address` with your Cardano node peer address. I can be either
> a local or remote node (i.e. any existing node relay).

## Overview

At present, most of the work around Amaru is spread across different repositories summarized in the table below. As an initial goal, we aim to consolidate and channel these workstreams into a single project.

 | Repository                                                      | Purpose                                                                                                                                               |
 | ---                                                             | ---                                                                                                                                                   |
 | [Pallas](https://github.com/txpipe/pallas)               | Hosts many Rust primitives and building blocks for the node already powering tools like Dolos. In particular, the networking and serialization logic. |
 | [pragma-org/ouroboros](https://github.com/pragma-org/ouroboros) | Rust libraries/building blocks to implement an Ouroboros (Praos & Genesis) consensus.                                                                 |
 | [pragma-org/uplc](https://github.com/pragma-org/uplc)           | A highly performant UPLC parser and CEK machine.                                                                                                      |


### Other relevant projects

While Amaru materialises a final binary executable, there is a variety of other
projects that are directly or indirectly relevant to Amaru. For example, Dolos
is a similar project with some overlap, but isn't directly used by Amaru.
Mithril on the other hand may become a first-class citizen in a later version of
Amaru.

To make things easier to follow, here's a list of relevant open source projects
in the context of Amaru:

| Repository                                                                                    | Relevance                                                                                                                                                                      |
| ---                                                                                           | ---                                                                                                                                                                            |
| [Dolos](https://github.com/txpipe/dolos)                                                      | A first incarnation of a node client, albeit geared towards client applications such as DApps. Dolos is de-facto a foundation which will inspire the design and work on Amaru. |
| [java-rewards-calculation](https://github.com/cardano-foundation/cf-java-rewards-calculation) | A Java re-implementation of the Cardano rewards calculation which can now serve as a reference implementation for a Rust version.                                              |
| [mithril](https://github.com/input-output-hk/mithril)                                         | A stake-based threshold multi-signatures protocol.                                                                                                                             |
| [cardano-multiplatform-library](https://github.com/dcSpark/cardano-multiplatform-lib)         | A Rust implementation of various Cardano data and crypto primitives.                                                                                                           |
| [aiken-lang/uplc](https://github.com/aiken-lang/aiken/tree/main/crates/uplc)                  | An Untyped Plutus Core (UPLC) parser and CEK machine for evaluating Plutus V2 and Plutus V3 on-chain scripts.                                                                  |
| [cncli](https://github.com/cardano-community/cncli)                                           | A Rust implementation of Cardano CLI tools including VRF functionality, and some consensus tooling like leaderlogs.                                                            |
| [gouroboros](https://github.com/blinklabs-io/gouroboros)                                      | Go implementation of the Cardano Ouroboros family of protocols                                                                                                                 |

<hr/>

<p align="center">
  :boat: <a href="https://github.com/orgs/pragma-org/projects/1/views/1">Roadmap</a>
  |
  :triangular_ruler: <a href="CONTRIBUTING.md">Contributing</a>
  |
  <a href="https://discord.gg/3nZYCHW9Ns"><img src=".github/discord.svg" alt="Discord" /> Discord</a>
</p>
