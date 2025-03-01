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

See [snapshots.json](snapshots.json). To download all available snapshots, uncompressed, in format suitable for `import`ing in Amaru, one can do:

```.bash
mkdir -p snapshots;
curl -s -o - "https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/data/snapshots.json" \
  | jq -r '.[] | "\(.point)  \(.url)"' \
  | while read p u ; do  \
      echo "Fetching $p.cbor"; \
      curl --progress-bar -o - $u \
        | gunzip > snapshots/$p.cbor ; \
    done
```
