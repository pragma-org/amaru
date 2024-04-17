# Amaru

Amaru is a [Cardano](https://cardano.org) node client written in Rust. It is an ambitious project which aims to bring more diversity to the infrastructure operating the Cardano network.

## Overview

At present, most of the work around Amaru is spread across different repositories summarized in the table below. As an initial goal, we aim to consolidate and channel these workstreams into a single project.

| Repository                                                                                    | Purpose                                                                                                                                                                        |
| ---                                                                                           | ---                                                                                                                                                                            |
| [Dolos](https://github.com/txpipe/dolos)                                                      | A first incarnation of a node client, albeit geared towards client applications such as DApps. Dolos is de-facto a foundation which will inspire the design and work on Amaru. |
| [Pallas](https://github.com/txpipe/pallas)                                                    | Hosts many Rust primitives and building blocks for the node already powering tools like Dolos. In particular, the networking and serialization logic.                          |
| [java-rewards-calculation](https://github.com/cardano-foundation/cf-java-rewards-calculation) | A Java re-implementation of the Cardano rewards calculation which can now serve as a reference implementation for a Rust version.                                                |
| [uplc](https://github.com/aiken-lang/aiken/tree/main/crates/uplc)                             | An Untyped Plutus Core (UPLC) parser and CEK machine for evaluating Plutus V2 and Plutus V3 on-chain scripts.                                                                  |
| [mithril](https://github.com/input-output-hk/mithril)                                         | A stake-based threshold multi-signatures protocol.                                                                                                                             |
| [cardano-multiplatform-library](https://github.com/dcSpark/cardano-multiplatform-lib)         | A Rust implementation of various Cardano data and crypto primitives.                                                                                                          |
<hr/>

<p align="center">
  :boat: <a href="https://github.com/orgs/pragma-org/projects/1/views/1">Roadmap</a>
  |
  :triangular_ruler: <a href="CONTRIBUTING.md">Contributing</a>
  |
  <a href="https://discord.gg/3nZYCHW9Ns"><img src=".github/discord.svg" alt="Discord" /> Discord</a>
</p>
