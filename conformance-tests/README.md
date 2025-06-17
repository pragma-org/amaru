# Conformance Tests

This folder contains scripts used to generate snapshots used for [conformance tests](../crates/amaru/tests/snapshots).

## Getting started

Install the required dependencies by running:

```console
yarn
```

If you haven't done so, you will also need [the necessary `data`](../data). For an epoch `n`, you need data corresponding to `n`, `n+1` and `n+3` in order to generate the test snapshots. So make sure that you have fetched all the necessary informations beforehand.

## Generating snapshots

Once the data is available, it needs some post-processing to create proper JSON snapshots to be used as conformance tests. The post-processing ensures that data is properly formatted in a canonical way, and combine raw data fetched from the node in a more meaningful way.

Simply run (replacing `NETWORK` with the target network):

```console
yarn generate-all NETWORK
```

> [!TIP]
>
> You can also generate snapshots for a single epoch:
>
> ```console
> yarn generate NETWORK 168
> ```
