---
id: amaru-user-guide
title: "Guide: How to Run an Amaru Node"
sidebar_label: "Guide: Running an Amaru Node"
description: A structured guide to installing, configuring, running, and maintaining an Amaru node.
---

## About this guide

This guide is built with the same Docusaurus setup and conventions as [developers.cardano.org/docs/operators](https://developers.cardano.org/docs/operators/), since parts of it are expected to be merged into that site at some point. Starting from the same template now means less rework when that happens.  
It is readable as a website here: https://arwlf.github.io/amaru-operator-portal

:::important Scope: block production is a testnet trial
On preprod, preview, and other testnets, `amaru node run` forges blocks when given a cardano-cli KES signing key, VRF signing key, and operational certificate. Mainnet refuses those flags. With no key files the node is a follower. There is no key generation or stake pool registration.
Track further block-producing support on the [Amaru roadmap](https://github.com/orgs/pragma-org/projects/3).

:::

This guide has two paths, depending on what you're trying to do:

- **[Fast-Forward on Preprod](01-amaru-fast-forward.md)** — the quickest way to get a synced Amaru node up for testing and development. Installs the binary, bootstraps from a snapshot, starts the node, enjoy **[the Amaru TUI](02-amaru-tui.md)**.
- **[Advanced Installation on Mainnet](03-amaru-advanced.md)** — a production-grade setup for operating Amaru on mainnet, in two parts:
  - **[System Hardening](04-amaru-advanced-hardening.md)**
  - **[Advanced Installation](05-amaru-advanced-installation.md)**

