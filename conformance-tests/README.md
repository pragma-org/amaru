# Conformance Tests

This folder contains scripts used to generate snapshots used for [conformance tests](../crates/amaru/tests/snapshots).

## Pre-Requisites

### 3rd-party dependencies

- [Ogmios](https://github.com/CardanoSolutions/ogmios) >= `v6.13.0`
- [cardano-node](https://github.com/IntersectMBO/cardano-node) >= `10.1.0`

### Node.js dependencies

Install necessary Node.js dependencies via:

```console
yarn
```

## Configuration

Data will be fetched for each point listed in [`config.json`](./config.json). Typically, those points needs to be the _last point_ of target epochs. Currently, it contains points for the PreProd network.

## Fetching Data

The Haskell node only has a memory of up to few thousand blocks. So, in order to fetch states corresponding to historical data, fetching must be done while the cardano-node is synchronizing.

For the current configuration, that means removing all immutable chunks after `03149`, starting your node from there will have it synchronize through the required points. As soon as a point gets available, data will be fetched and the script will move on to the next point. Once your Haskell node is syncing and Ogmios up-and-running as well, quick run:

```console
yarn fetch
```

This should terminate once all the points have been processed.

## Generating snapshots

Once the data is available, it needs some post-processing to create proper JSON snapshots to be used as conformance tests. The post-processing ensures that data is properly formatted in a canonical way, and combine raw data fetched from the node in a more meaningful way.

Simply run:

```console
yarn generate-all
```

> [!TIP]
>
> You can also generate snapshots for a single epoch:
>
> ```console
> yarn generate 168
> ```
