# Overview

## Cardano ledger snapshots

We provide CBOR-serialised snapshots obtained from a running Haskell Cardano node client. More specifically, the snapshots are obtained using the `GetCBOR` ledger-state query, and dumped as such to disk.

> [!NOTE]
> It would be nice to have a clear specification for the snapshot format? Wouldn't it? Well, that format isn't quite documented because it's based off the internal object representations (specifically, the `EpochState` type) from the Haskell codebase, and there's no commitment to maintaining compatibility to that format across updates. So at this point, the best reference is [the Haskell source code](https://github.com/IntersectMBO/cardano-ledger/blob/33e90ea03447b44a389985ca2b158568e5f4ad65/eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs#L121-L131).
>
> If someone finds the motivation to document as CDDL the encoders from that pinned reference, a PR is more than welcome.

Snapshots are named after the point on chain they've been taken from. More specifically, the first characters represent an absolute slot number, followed by 64 characters representing the hex-encoded block header hash; both separated by a period `.`. For example:

```
68774372.36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor
```

designates the ledger state dump of the last block of the Conway era (including the processing of that block) on the PreProd network.

### Available snapshots

See [$NETWORK/snapshots.json](.). To download all available snapshots, uncompressed, in format suitable for `import`ing in Amaru, one can do:

```.bash
mkdir -p snapshots/${NETWORK};
curl -s -o - "https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/data/${NETWORK}/snapshots.json" \
  | jq -r '.[] | "\(.point)  \(.url)"' \
  | while read p u ; do  \
      echo "Fetching $p.cbor"; \
      curl --progress-bar -o - $u \
        | gunzip > snapshots/${NETWORK}/$p.cbor ; \
    done
```

## Fetching Ledger Data

### Pre-Requisites

#### 3rd-party dependencies

- [Ogmios](https://github.com/CardanoSolutions/ogmios) >= `v6.13.0`
- [cardano-node](https://github.com/IntersectMBO/cardano-node) >= `10.1.0`

#### Node.js dependencies

Install necessary Node.js dependencies via:

```console
yarn
```

### Configuring

Data will be fetched for each point listed network's configurations `points` field. Full snapshots can also be fetched along the way by specifying epoch numbers as `snapshots`.

- [`preview/config.json`](./preview/config.json)
- [`preprod/config.json`](./preprod/config.json)
- [`mainnet/config.json`](./mainnet/config.json)

> [!IMPORTANT]
> Those points needs to be the _last point_ of target epochs.

### Running

The Haskell node only has a memory of up to few thousand blocks. So, in order to fetch states corresponding to historical data, fetching must be done while the cardano-node is synchronizing.

For example, for PreProd, that means removing all immutable chunks after `03149`, starting your node from there will have it synchronize through the required points. As soon as a point gets available, data will be fetched and the script will move on to the next point. So, once your node is ready:

1. Run ogmios

2. Run the fetch script (replacing `NETWORK` with the name of the target network):

   ```console
   yarn fetch NETWORK
   ```

   // or alternatively, also fetch and dump the full snapshots:

   ```console
   yarn fetch NETWORK true
   ```

3. Start your node.

Both Ogmios and the fetch script will automatically connect to the node once ready and capture data from it when it becomes available. This should terminate once all the points have been processed.

4. Dump chain headers as CBOR

From the repository root, run:

```bash
make fetch-chain-headers
```

This will add `header.<slot>.<hash>.cbor` files to `data/<network>/headers`, which the bootstrap command consumes to initialize the Amaru node.
