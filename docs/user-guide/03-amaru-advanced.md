---
id: amaru-advanced
title: "Running an Amaru Node on Mainnet (Advanced Installation)"
sidebar_label: "Advanced Installation on Mainnet"
description: Prerequisites for a production-grade Amaru deployment on mainnet, split into system hardening and advanced installation.
---

This is the production-grade path for running Amaru on mainnet: hardening the host, installing Amaru as a systemd service under its own dedicated system user, and operating it long-term. It's split into two parts:

1. [System Hardening](04-amaru-advanced-hardening.md) — secures the host before anything is installed.
2. [Advanced Installation](05-amaru-advanced-installation.md) — installs Amaru, configures it for mainnet, and covers day-2 operations.

:::tip Just want to test on preprod?
This path isn't needed for a quick test — see [Running on Preprod (Fast-Forward)](01-amaru-fast-forward.md) instead.
:::

## Hardware requirements

| Network | CPU Cores | Free RAM | Free storage |
|:-------:|:---------:|:--------:|:-------------:|
| Mainnet |     2     |   4GB    |     30GB      |
| Preprod |     2     |   2GB    |     10GB      |

:::note
The Amaru process should not exceed 1GB of RSS memory under standard conditions. Allocating 4GB of RAM provides sufficient headroom for the process to handle the load associated with mainnet stress.
:::
