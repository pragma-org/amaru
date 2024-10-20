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
