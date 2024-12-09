# Overview

The following folders contain pieces of data needed to run the Amaru demo. Steps to get those pieces of data are detailed below. In the long run, those steps shall become redundant as features gets integrated inside Amaru.

## Fetching stake distribution snapshots

### Using [`cncli`](https://github.com/cardano-community/cncli)

While in epoch `e`, we can get fetch the _go_ snapshot corresponding to the stake distribution of epoch `e - 1`. So for example, if a node is in epoch 173, we can get the stake distribution of epoch 172 (on preprod) as follows:

```console
cncli pool-stake --socket-path /path/to/node.socket --name go --output-file preprod_172_stake_snapshot.csv --network-magic 1
```

> ![NOTE]
> The last block of epoch 171 is 72662384.2f2bcab30dc53444cef311d6985eb126fd577a548b8e71cafecbb6e855bc8755

## Fetching epoch nonces (CSV)

### From Koios

```bash
#!/bin/bash

rm -f preprod_nonce.csv
touch preprod_nonce.csv

# loop from 4 to 174 inclusive
for i in {4..174}
do
    nonce=$(curl -X GET "https://preprod.koios.rest/api/v1/epoch_params?_epoch_no=$i" -H "accept: application/json" 2>/dev/null | jq -r '.[0].nonce')
    echo "$i,$nonce" >> preprod_nonce.csv
    echo "Epoch $i nonce: $nonce"
done
```

## Cardano ledger snapshots

In addition, we provide CBOR-serialised snapshots obtained from a running Haskell Cardano node client. More specifically, the snapshots are obtained using the `GetCBOR` ledger-state query, and dumped as such to disk.

> [!NOTE]
> It would be nice to have a clear specification for the snapshot format? Wouldn't? Well, that format isn't quite documented because it's based off the internal object representations (specifically, the `EpochState` type) from the Haskell codebase, and there's no commitment to maintaining compatibility to that format across updates. So at this point, the best reference is [the Haskell source code](https://github.com/IntersectMBO/cardano-ledger/blob/33e90ea03447b44a389985ca2b158568e5f4ad65/eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs#L121-L131).
>
> If someone finds the motivation to document as CDDL the encoders from that pinned reference, a PR is more than welcome.

Snapshots are named after the point on chain they've been taken from. More specifically, the first characters represent an absolute slot number, followed by 64 characters representing the hex-encoded block header hash; both separated by a dash `-`. For example:

```
68774372-36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor
```

designates the ledger state dump of the last block of the Conway era (including the processing of that block) on the PreProd network.


### Available snapshots

#### PreProd

 | Download link | Size |
 | :---           | :---  |
| [68774372-36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor](https://pub-b844360df4774bb092a2bb2043b888e5.r2.dev/68774372-36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor.gz) | `203.12 MB` |
